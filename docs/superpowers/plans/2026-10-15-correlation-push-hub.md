# Correlation history, Hub and lab side, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** PerfSentinelHub stores the correlation episodes a daemon pushes to `POST /api/import/correlations?source_id=<id>`, serves them on `GET /api/correlations`, and shows them on a Correlations screen and in an incident's detail. A lab scenario proves a pair active twice lands as two episodes, the second overlapping an incident.

**Architecture:** A third import route beside the findings and incidents imports, sharing their admission (query check, `X-API-Key` against the source's `ImportApiKey`, the 4-slot `ImportGate`, the 5 s write wait). A parser checks each episode alone, a new `correlation_episodes` table merges copies under an order-free rule, and the daily purge drops episodes on `last_seen_ms`. The read route reuses the incidents' access scoping and applies a stale rule at read time. The launcher groups episodes into pairs client-side, draws a strip per pair, and adds a block to an incident's detail. The lab drives an isolated daemon and Hub with synthetic N+1 traces.

**Tech Stack:** .NET 10 NativeAOT minimal API, SQLite (Microsoft.Data.Sqlite), xunit v3 on Microsoft.Testing.Platform with `HubApplicationFactory`, vanilla JS launcher with `node --test`, Playwright demo suite, bash + python3 lab scenario on k3d.

**Spec:** `perf-sentinel/docs/superpowers/specs/2026-10-15-correlation-history-in-hub-design.md` (engine repo). This plan implements its **Hub** section, the Hub lines of **Documentation** and **Testing**, and the lab scenario of **Testing**. The engine side and the Grafana panel belong to another plan. The wire format and the daemon's behaviour are fixed by the spec.

**Repositories:**
- Hub: `/Users/robintrassard/RiderProjects/PerfSentinelHub`, new branch `feat/correlation-import` cut from `main` (Task 1, Step 0).
- Lab: `/Users/robintrassard/IdeaProjects/perf-sentinel-simulation-lab`, commits straight on `main`, no branch, no PR (Task 11).

**Running .NET tests.** `dotnet test ... -- -class X` reports zero tests on this setup. Build, then run the test host directly, as the Hub's `CLAUDE.md` says:

```bash
export DOTNET_ROOT=$HOME/.dotnet PATH=$HOME/.dotnet:$PATH
dotnet build PerfSentinelHub.Tests/PerfSentinelHub.Tests.csproj -c Release --nologo -v q
./PerfSentinelHub.Tests/bin/Release/net10.0/PerfSentinelHub.Tests -class PerfSentinelHub.Tests.<ClassName>
```

Every `Run:` line below that names a test class means these three lines with that class.

## Global Constraints

- SDK pinned 10.0.401 lives in `~/.dotnet`: `export DOTNET_ROOT=$HOME/.dotnet PATH=$HOME/.dotnet:$PATH` before any `dotnet` or `make` command.
- `make verify-fast` green before handing back (build `--warnaserror`, `dotnet format --verify-no-changes`, Python tests, JS tests, coverage ratchet with 80% on new code, analysis-config and badge checks). `make format` is the verify-only format check: run `dotnet format PerfSentinelHub.sln` first, then `make format`.
- NativeAOT: no reflection serialization. Every serialized type goes through `Api/HubJsonContext.cs` (snake_case), or a hand-written `Utf8JsonWriter`.
- Braces on a `foreach` or `for` nested in another loop or block (Rider and Roslyn disagree on brace-less nesting). New code puts braces on every loop body.
- Route: `POST /api/import/correlations?source_id=<id>`, `.AllowAnonymous()`, same query validation as the two other imports (valid UTF-8, exactly one `source_id`, non-empty).
- Auth: `X-API-Key` compared to `SourceOptions.ImportApiKey` with the existing `IsAuthorized` (SHA-256 then `FixedTimeEquals`). Unknown source or wrong key: 401.
- Gate: the existing `ImportGate` (4 slots), shared with the findings and incident imports. Full: 503 with `Retry-After: 1`.
- Body cap: 1 MiB (`1024 * 1024`), else 413. Envelope `{"producer_version": "<1..64 chars>", "episodes": [1..100 episodes]}`, duplicate properties refused (`JsonRead.UniqueProperties`). More than 100: 413. Zero, or none accepted: 400.
- Each episode validated alone: `id` and `pair_key` 32 lowercase hex, `finding_type` of both sides in `FindingLabels.Types`, non-empty `service`, string `template`, `started_at_ms <= last_seen_ms`, `ended_at_ms` absent, null or `>= started_at_ms`, `peak_confidence` and `last_confidence` in `[0, 1]`, co-occurrence counts `>= 0`, `gap_ms > 0`. An invalid one is rejected alone and counted in `rejected`.
- Write: wait at most 5 s (`WriteGateWait`) for the write gate, else 503 `Retry-After: 1`. Upsert `source_imports.last_import_ms`. Never touch `source_state`.
- Merge rule: `started_at_ms` takes the minimum, `last_seen_ms` and both peaks the maximum, the rolling values (`last_confidence`, `last_co_occurrences`, `median_lag_ms`, both sample trace ids, `gap_ms`) come from the copy with the newer `last_seen_ms`, `ended_at_ms` is kept once seen.
- Response: 200 `{"accepted": n, "rejected": m}` (the existing `ImportResponse`) after the commit.
- Metric: `perf_sentinel_hub_correlation_import_rejected_total{reason}` with reasons `bad_request`, `unauthorized`, `gate_full`, `write_timeout`, `too_large`, all published from startup.
- Log: event 1303 at Warning, "Source {SourceId} rejected {RejectedCount} imported correlation episodes.", declared in `ApiEndpoints.cs` beside 1301 and 1302 (`LoggerMessageLayoutTests` requires one file per class).
- Table `correlation_episodes`, key `(id, source_id)`, indexes on `(started_at_ms DESC)`, `(source_id, pair_key, started_at_ms)` and `(last_seen_ms)`. Daily purge on `last_seen_ms` older than `Hub:Retention` (180 days by default). No new option.
- Each side on the wire is the correlator's `CorrelationEndpoint`: `finding_type`, `service`, `template`, plus `grouping_key` and `grouping_value`, both omitted when absent and read as null. Both grouping fields are stored, in `*_grouping` and `*_grouping_value`.
- `GET /api/correlations`: a bare JSON array of episodes in the push shape plus `source_id` and `source_name`, and `ended_by` set to `"stale"` only when the stale rule applies (null otherwise). Access scoping of `GET /api/incidents` (`findings.read`, 403 when held nowhere). Filters `source_id`, `environment`, `service` (either side), `pair_key`, `from` and `to` (overlap, open episodes included), `active`, `offset`, `limit` (200 by default, 1000 at most). A filter given empty or as whitespace only means no filter (Grafana sends `service=` and `source_id=` for All). Order `started_at_ms` descending. Stale rule: an open episode whose `last_seen_ms` is older than `gap_ms` plus 15 minutes is served as ended at its `last_seen_ms` with `"ended_by": "stale"`, and the row is not rewritten.
- The launcher opens `#/correlations?pair_key=<key>` filtered on that pair, which the Grafana table links to.
- Launcher: tab Correlations after Incidents, `#/correlations`, heading "Findings that keep following each other", filters range (last 24 hours by default), service, source daemon, "active now only". One row per pair, unfold = strip with the daemon's incidents in red plus the episodes table with "Analyse this window" and "Explain the last pair". In an incident's detail, the block "Correlations active during this incident" reading `GET /api/correlations?source_id=…&from=window_from_ms&to=(ended_at_ms ?? window_to_ms)`, with the empty states "No correlation was active in this window." and "<Source> pushes no correlations, so none can show here." Reuse existing launcher classes and tokens, no new visual world.
- Screenshots regenerated with `npm run demo` when a screen changes: `cd tests/browser && DOTNET_ROOT="$HOME/.dotnet" npm run demo` (needs `target/release/perf-sentinel` built in the engine checkout).
- English only in code, comments, docs, UI strings. Every `docs/X.md` change ships with `docs/FR/X-FR.md` in the same commit. Prose: no em dashes, no semicolons, straight quotes, French with its accents.
- Commits: Conventional Commits, subject line only, no body, no Co-Authored-By, no process words, signed (`-S`), chained with `&&` after the checks. `main` is protected: PR with required checks, merged by merge commit.
- Lab: never commit the image pins `make seed-daemon-local` and `make seed-hub-local` write into `manifests/`. Stage the listed paths only.
- After code changes run `graphify update .` in the repository touched (`graphify-out/` is gitignored in both).
- Hub version 0.6.0 is the release that ships this. The release itself (version bump, ledger, tag) is out of this plan.

## Review Focus

- A close copy and an update copy of one episode carrying the same `last_seen_ms` (a close is queued at the last co-occurrence, with rolling values that drifted since) must converge on the close copy's values in either order. Pinned in Task 2.
- An open episode served as `stale` must not be excluded forever: the row keeps no end, so a late close from a daemon that came back lands and clears `ended_by`. Pinned in Task 3.
- The overlap filter's bounds are inclusive and a running episode reaches every later moment: an episode that ended exactly at `from`, one that started exactly at `to`, and a running one all match. Pinned in Task 3.
- With access control on, an episode whose target service the member cannot read must stay hidden even when its source service is readable, since it names both services and both templates. Pinned in Task 6.
- On a 390 px phone with a pair unfolded, only the table scrolls sideways, never the page. Pinned in Task 8.

## Rulings

Where the spec is silent or the code makes a literal reading impossible, this plan decides:

1. **Access, both sides.** An episode is listed only when one pattern reads its source and source service and one reads its source and target service. A pattern keyed on an attribute never reads an episode, since an episode carries no attribute.
2. **Range filter.** The New analysis range picker is bound to `state.form`, so the Correlations screen offers a select over fixed relative ranges `1h, 6h, 24h, 7d, 30d, 180d`, last 24 hours by default, drawn like its other filter selects, as the mockup draws it. No absolute range.
3. **`ended_by`.** `stale` when the Hub serves an open episode as ended, null otherwise, a closed episode being told by its `ended_at_ms` alone. Fixed by the engine plan's ruling 2 (the Grafana panels read `ended_by` as the stale flag).
4. **"Explain the last pair".** The Hub has no explain route. The link opens New analysis in Trace ID mode on the first finding's sample trace (the second's when the first is missing), and the banner offers the other trace with one click. It shows only when the Hub has a trace backend source and the episode has a sample trace.
5. **Tie-break in the merge rule.** At equal `last_seen_ms` the copy carrying an end is the newer one, otherwise two copies of one moment keep whichever arrived first.
6. **Purge clock.** `last_seen_ms`, the daemon's stamp, as the spec says, not the Hub clock incidents are purged on.
7. **Read limits.** The spec's constants, 200 and 1000, not `Hub:DefaultReadLimit` and `Hub:MaxReadLimit`.
8. **Sample trace ids.** Optional, at most 64 characters, an over-long one rejects its episode (it travels into a link). `producer_version` is checked (1 to 64 characters) and not stored.
9. **Incident links.** An overlapped incident opens the incidents screen filtered on its daemon with that incident unfolded, since that screen has no deep link. "All its episodes" links to `#/correlations?pair_key=<key>`, the route the Grafana table uses: it reads the pair over the widest range (180 days) and unfolds it.
10. **Incidents on the Correlations screen.** Read from the store (`GET /api/incidents?limit=<page>&offset=0`), never by a fleet refresh, and drawn when they belong to the pair's daemon and the range.
11. **Embed fixture.** The correlations reads carry a time-dependent `from`, so `hub-embed.json` records `/api/correlations` without a query. The website's replay must strip the query of that route to serve it, a follow-up in the site repository.
12. **Shared body reading.** A third handler would repeat the incident import's read-and-parse block word for word, so both use one `ReadImportAsync<T>` helper. The findings handler is left as it is.
13. **S107 on `MetricsEndpoint`.** The extra metrics collaborator takes `MapMetrics`' lambda and `RenderAsync` to nine parameters, wrapped in the `#pragma warning disable S107` the other injected endpoints use.
14. **Lab pair.** No shop fault endpoint reaches an isolated daemon: the collector exports to `perf-sentinel-daemon.observability` only, and the shop's correlated faults (`scenarios/n-plus-one-sql.js` on `order-service /api/fault/n-plus-one-sql`, `scenarios/pool-saturation.js` on `/api/fault/pool-saturation`) only ever reach the shared daemon. The scenario therefore sends the pair `correlation-event-time` already proved: `tools/tracegen/emit_at.py --shape n_plus_one` in `corpush-order` (table `orders`), followed 1 s later in event time by the same shape in `corpush-payment` (table `payments`), yielding `n_plus_one_sql/corpush-order` then `n_plus_one_sql/corpush-payment`.
15. **Lab scenario count.** The help text and `docs/SCENARIOS.md` say 95 while the `verify-all-scenarios` list already holds 99. They become 100. The lab's `CLAUDE.md` also says 95 and is left to the owner.
16. **Grouping on the wire (engine plan ruling 1).** Each side is the correlator's `CorrelationEndpoint` with `grouping_key` and `grouping_value` omitted when absent, so the spec's `null` example is not literal. A missing field reads as null, `EpisodeSide` carries both, and the table gains `source_grouping_value` and `target_grouping_value` beside `*_grouping`. `pair_key` hashes all five fields, so one pair in two namespaces is two keys, which the Hub takes as given.
17. **Read shape (engine plan ruling 2).** `GET /api/correlations` is a bare array in the push shape plus `source_id` and `source_name`, `ended_by` only for the stale rule. The Hub also serves `first_seen_ms` and `last_seen_hub_ms`, which nothing on the engine side reads. An open episode is served with `ended_at_ms: null`, and every consumer here (launcher, lab) also accepts it absent.
18. **Blank filters (engine plan ruling 3).** An empty or whitespace-only `service`, `source_id`, `environment`, `pair_key`, `from`, `to` or `active` means no filter. `limit` and `offset` keep the incidents' rule (a blank one is a `400`), since they are paging, not filters, and the shared `TryReadBounded` serves both routes.
19. **Pair route (engine plan ruling 4).** `#/correlations?pair_key=<key>` opens the screen filtered on that pair over 180 days with its rows unfolded. The plain `#/correlations` route drops the pair filter, so the tab always shows every pair.

---

### Task 1: Parse the correlation import envelope

**Files:**
- Create: `PerfSentinelHub/Collection/CorrelationParser.cs`
- Create: `PerfSentinelHub.Tests/EpisodeJson.cs` (test helper shared by Tasks 1 to 6)
- Test: `PerfSentinelHub.Tests/CorrelationParserTests.cs` (create)

**Interfaces:**
- Consumes: `JsonRead.ReadString`, `JsonRead.ReadLong`, `JsonRead.ReadEpochMs`, `JsonRead.UniqueProperties` (`Api/JsonPrimitives.cs`), `FindingLabels.Types` (`Storage/StorageModels.cs`), `ImportBatchTooLargeException` (`Collection/FindingParser.cs`).
- Produces:
  - `public sealed record EpisodeSide(string FindingType, string Service, string Template, string? GroupingKey, string? GroupingValue);`
  - `public sealed record ParsedEpisode(string Id, string PairKey, EpisodeSide Source, EpisodeSide Target, long StartedAtMs, long LastSeenMs, long? EndedAtMs, long GapMs, double PeakConfidence, long PeakCoOccurrences, double LastConfidence, long LastCoOccurrences, double? MedianLagMs, string? SampleTraceId, string? SourceSampleTraceId);`
  - `public sealed record ParsedEpisodeImport(string ProducerVersion, IReadOnlyList<ParsedEpisode> Episodes, int RejectedCount);`
  - `public static ParsedEpisodeImport CorrelationParser.ParseImport(ReadOnlyMemory<byte> payload)`, throwing `InvalidDataException` for a malformed envelope or an empty `episodes`, and `ImportBatchTooLargeException` above `CorrelationParser.MaxImportEpisodes` (100).
  - Tests: `EpisodeJson.PairKey`, `EpisodeJson.Id(int)`, `EpisodeJson.Episode(...)`, `EpisodeJson.Body(params JsonObject[])`, `EpisodeJson.Parsed(params JsonObject[])`.

- [ ] **Step 0: Cut the branch**

```bash
cd /Users/robintrassard/RiderProjects/PerfSentinelHub && git switch main && git pull --ff-only && git switch -c feat/correlation-import
```

- [ ] **Step 1: Write the test helper**

Create `PerfSentinelHub.Tests/EpisodeJson.cs`:

```csharp
using System.Globalization;
using System.Text;
using System.Text.Json.Nodes;
using PerfSentinelHub.Collection;

namespace PerfSentinelHub.Tests;

/// <summary>An episode as a daemon pushes it, each field valid until a test overrides it.</summary>
internal static class EpisodeJson
{
    public const string PairKey = "0123456789abcdef0123456789abcdef";

    public static string Id(int index)
    {
        return index.ToString("x32", CultureInfo.InvariantCulture);
    }

    public static JsonObject Episode(
        string id,
        long startedAtMs = 1_800_000_000_000,
        long lastSeenMs = 1_800_000_060_000,
        long? endedAtMs = null,
        string pairKey = PairKey,
        string sourceService = "order-svc",
        string targetService = "payment-svc")
    {
        return new JsonObject
        {
            ["id"] = id,
            ["pair_key"] = pairKey,
            ["source"] = Side("n_plus_one_sql", sourceService, "SELECT * FROM orders WHERE id = ?"),
            ["target"] = Side("pool_saturation", targetService, "SELECT 1"),
            ["started_at_ms"] = startedAtMs,
            ["last_seen_ms"] = lastSeenMs,
            ["ended_at_ms"] = endedAtMs,
            ["peak_confidence"] = 0.92,
            ["peak_co_occurrences"] = 41,
            ["last_confidence"] = 0.88,
            ["last_co_occurrences"] = 37,
            ["median_lag_ms"] = 1200.0,
            ["sample_trace_id"] = "4bf92f3577b34da6a3ce929d0e0e4736",
            ["source_sample_trace_id"] = "00f067aa0ba902b74bf92f3577b34da6",
            ["gap_ms"] = 600_000
        };
    }

    public static string Body(params JsonObject[] episodes)
    {
        return new JsonObject
        {
            ["producer_version"] = "0.30.0",
            ["episodes"] = new JsonArray([.. episodes.Select(episode => (JsonNode?)episode.DeepClone())])
        }.ToJsonString();
    }

    public static IReadOnlyList<ParsedEpisode> Parsed(params JsonObject[] episodes)
    {
        return CorrelationParser.ParseImport(Encoding.UTF8.GetBytes(Body(episodes))).Episodes;
    }

    // As the correlator writes an endpoint: the two grouping fields are omitted when absent.
    private static JsonObject Side(string type, string service, string template)
    {
        return new JsonObject { ["finding_type"] = type, ["service"] = service, ["template"] = template };
    }
}
```

- [ ] **Step 2: Write the failing tests**

Create `PerfSentinelHub.Tests/CorrelationParserTests.cs`:

```csharp
using System.Text;
using System.Text.Json.Nodes;
using PerfSentinelHub.Collection;

namespace PerfSentinelHub.Tests;

public sealed class CorrelationParserTests
{
    private static ParsedEpisodeImport Parse(params JsonObject[] episodes)
    {
        return CorrelationParser.ParseImport(Encoding.UTF8.GetBytes(EpisodeJson.Body(episodes)));
    }

    [Fact]
    public void An_episode_parses_with_both_sides_and_its_figures()
    {
        var import = Parse(EpisodeJson.Episode(EpisodeJson.Id(1)));

        Assert.Equal("0.30.0", import.ProducerVersion);
        Assert.Equal(0, import.RejectedCount);
        var episode = Assert.Single(import.Episodes);
        Assert.Equal(EpisodeJson.Id(1), episode.Id);
        Assert.Equal(EpisodeJson.PairKey, episode.PairKey);
        Assert.Equal(new EpisodeSide("n_plus_one_sql", "order-svc", "SELECT * FROM orders WHERE id = ?", null, null),
            episode.Source);
        Assert.Equal("pool_saturation", episode.Target.FindingType);
        Assert.Equal(1_800_000_000_000, episode.StartedAtMs);
        Assert.Null(episode.EndedAtMs);
        Assert.Equal(600_000, episode.GapMs);
        Assert.Equal(0.92, episode.PeakConfidence);
        Assert.Equal(41, episode.PeakCoOccurrences);
        Assert.Equal(1200.0, episode.MedianLagMs);
        Assert.Equal("00f067aa0ba902b74bf92f3577b34da6", episode.SourceSampleTraceId);
    }

    [Fact]
    public void A_side_keeps_its_grouping_key_and_value()
    {
        var grouped = EpisodeJson.Episode(EpisodeJson.Id(1));
        grouped["target"]!["grouping_key"] = "k8s.namespace.name";
        grouped["target"]!["grouping_value"] = "payments";

        var episode = Assert.Single(Parse(grouped).Episodes);

        Assert.Equal("k8s.namespace.name", episode.Target.GroupingKey);
        Assert.Equal("payments", episode.Target.GroupingValue);
        Assert.Null(episode.Source.GroupingValue);
    }

    [Fact]
    public void An_absent_end_reads_as_an_open_episode()
    {
        var open = EpisodeJson.Episode(EpisodeJson.Id(1));
        open.Remove("ended_at_ms");

        Assert.Null(Assert.Single(Parse(open).Episodes).EndedAtMs);
    }

    [Theory]
    [InlineData("id", "\"not-a-hex-id\"")]
    [InlineData("id", "\"0123456789ABCDEF0123456789ABCDEF\"")]
    [InlineData("pair_key", "\"0123\"")]
    [InlineData("started_at_ms", "1800000070000")]
    [InlineData("ended_at_ms", "1799999999999")]
    [InlineData("peak_confidence", "1.01")]
    [InlineData("last_confidence", "-0.1")]
    [InlineData("peak_co_occurrences", "-1")]
    [InlineData("last_co_occurrences", "\"37\"")]
    [InlineData("gap_ms", "0")]
    [InlineData("source", "\"order-svc\"")]
    public void An_invalid_episode_is_rejected_alone(string field, string json)
    {
        var invalid = EpisodeJson.Episode(EpisodeJson.Id(2));
        invalid[field] = JsonNode.Parse(json);

        var import = Parse(EpisodeJson.Episode(EpisodeJson.Id(1)), invalid);

        Assert.Equal(EpisodeJson.Id(1), Assert.Single(import.Episodes).Id);
        Assert.Equal(1, import.RejectedCount);
    }

    [Theory]
    [InlineData("source", "finding_type", "made_up")]
    [InlineData("target", "finding_type", "made_up")]
    [InlineData("target", "service", "")]
    public void A_side_the_engine_cannot_have_written_rejects_its_episode(string side, string field, string value)
    {
        var invalid = EpisodeJson.Episode(EpisodeJson.Id(2));
        invalid[side]![field] = value;

        Assert.Equal(1, Parse(EpisodeJson.Episode(EpisodeJson.Id(1)), invalid).RejectedCount);
    }

    [Fact]
    public void An_overlong_sample_trace_rejects_its_episode()
    {
        var invalid = EpisodeJson.Episode(EpisodeJson.Id(2));
        invalid["sample_trace_id"] = new string('a', CorrelationParser.MaxTraceIdLength + 1);

        Assert.Equal(1, Parse(EpisodeJson.Episode(EpisodeJson.Id(1)), invalid).RejectedCount);
    }

    [Fact]
    public void A_missing_sample_trace_is_kept_as_unknown()
    {
        var episode = EpisodeJson.Episode(EpisodeJson.Id(1));
        episode["sample_trace_id"] = null;

        Assert.Null(Assert.Single(Parse(episode).Episodes).SampleTraceId);
    }

    [Theory]
    [InlineData("[]")]
    [InlineData("{}")]
    public void An_import_without_episodes_is_invalid(string episodes)
    {
        Assert.Throws<InvalidDataException>(() => CorrelationParser.ParseImport(
            Encoding.UTF8.GetBytes($"{{\"producer_version\":\"0.30.0\",\"episodes\":{episodes}}}")));
    }

    [Fact]
    public void An_import_refuses_a_missing_or_overlong_producer_version()
    {
        var episodes = $"[{EpisodeJson.Episode(EpisodeJson.Id(1)).ToJsonString()}]";
        Assert.Throws<InvalidDataException>(() => CorrelationParser.ParseImport(
            Encoding.UTF8.GetBytes($"{{\"episodes\":{episodes}}}")));
        Assert.Throws<InvalidDataException>(() => CorrelationParser.ParseImport(
            Encoding.UTF8.GetBytes($"{{\"producer_version\":\"{new string('9', 65)}\",\"episodes\":{episodes}}}")));
    }

    [Fact]
    public void An_import_of_more_than_a_hundred_episodes_is_too_large()
    {
        var episodes = Enumerable.Range(1, CorrelationParser.MaxImportEpisodes + 1)
            .Select(index => EpisodeJson.Episode(EpisodeJson.Id(index)))
            .ToArray();

        Assert.Throws<ImportBatchTooLargeException>(() => Parse(episodes));
    }

    [Fact]
    public void An_import_repeating_a_property_is_invalid()
    {
        var episodes = $"[{EpisodeJson.Episode(EpisodeJson.Id(1)).ToJsonString()}]";
        var payload = Encoding.UTF8.GetBytes(
            $"{{\"producer_version\":\"0.30.0\",\"producer_version\":\"0.29.0\",\"episodes\":{episodes}}}");

        Assert.Throws<InvalidDataException>(() => CorrelationParser.ParseImport(payload));
    }

    [Fact]
    public void An_import_that_is_not_json_is_invalid()
    {
        Assert.Throws<InvalidDataException>(() => CorrelationParser.ParseImport(Encoding.UTF8.GetBytes("{\"episodes\":")));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: class `PerfSentinelHub.Tests.CorrelationParserTests` (build, then the test host)
Expected: build error CS0246, `CorrelationParser`, `ParsedEpisodeImport` and `EpisodeSide` could not be found.

- [ ] **Step 4: Implement the parser**

Create `PerfSentinelHub/Collection/CorrelationParser.cs`:

```csharp
using System.Text.Json;
using PerfSentinelHub.Api;
using PerfSentinelHub.Storage;

namespace PerfSentinelHub.Collection;

/// <summary>
///     One side of a correlated pair, the correlator's own endpoint. The two grouping fields
///     are omitted on the wire when the finding had no grouping, and read as null.
/// </summary>
public sealed record EpisodeSide(
    string FindingType,
    string Service,
    string Template,
    string? GroupingKey,
    string? GroupingValue);

/// <summary>
///     One period during which a pair of findings kept following each other, as its
///     daemon cut it. The daemon is the author: the Hub merges copies and re-derives nothing.
/// </summary>
public sealed record ParsedEpisode(
    string Id,
    string PairKey,
    EpisodeSide Source,
    EpisodeSide Target,
    long StartedAtMs,
    long LastSeenMs,
    long? EndedAtMs,
    long GapMs,
    double PeakConfidence,
    long PeakCoOccurrences,
    double LastConfidence,
    long LastCoOccurrences,
    double? MedianLagMs,
    string? SampleTraceId,
    string? SourceSampleTraceId);

public sealed record ParsedEpisodeImport(
    string ProducerVersion,
    IReadOnlyList<ParsedEpisode> Episodes,
    int RejectedCount);

public static class CorrelationParser
{
    /// <summary>A pushed batch holds at most this many episodes, the daemon's own batch size.</summary>
    public const int MaxImportEpisodes = 100;

    /// <summary>A sample trace id travels into a link, so a longer one is refused rather than stored.</summary>
    public const int MaxTraceIdLength = 64;

    public static ParsedEpisodeImport ParseImport(ReadOnlyMemory<byte> payload)
    {
        JsonDocument document;
        try
        {
            document = JsonDocument.Parse(payload, JsonRead.UniqueProperties);
        }
        catch (JsonException exception)
        {
            throw new InvalidDataException("The import body is not valid JSON.", exception);
        }

        using (document)
        {
            var root = document.RootElement;
            if (root.ValueKind != JsonValueKind.Object ||
                JsonRead.ReadString(root, "producer_version") is not { Length: > 0 and <= 64 } producerVersion ||
                !root.TryGetProperty("episodes", out var episodes) ||
                episodes.ValueKind != JsonValueKind.Array)
                throw new InvalidDataException("The import body is invalid.");

            var count = episodes.GetArrayLength();
            if (count > MaxImportEpisodes)
                throw new ImportBatchTooLargeException();
            if (count == 0)
                throw new InvalidDataException("The import batch is empty.");

            var parsed = new List<ParsedEpisode>(count);
            foreach (var element in episodes.EnumerateArray())
            {
                if (TryParse(element, out var episode))
                    parsed.Add(episode);
            }

            return new ParsedEpisodeImport(producerVersion, parsed, count - parsed.Count);
        }
    }

    // Each episode on its own: one the engine could not have written is refused and
    // counted, and its neighbours still land.
    private static bool TryParse(JsonElement element, out ParsedEpisode episode)
    {
        episode = null!;
        if (element.ValueKind != JsonValueKind.Object ||
            JsonRead.ReadString(element, "id") is not { } id || !IsHexId(id) ||
            JsonRead.ReadString(element, "pair_key") is not { } pairKey || !IsHexId(pairKey) ||
            ReadSide(element, "source") is not { } source ||
            ReadSide(element, "target") is not { } target ||
            JsonRead.ReadEpochMs(element, "started_at_ms") is not { } startedAtMs ||
            JsonRead.ReadEpochMs(element, "last_seen_ms") is not { } lastSeenMs ||
            startedAtMs > lastSeenMs ||
            !TryReadEnd(element, startedAtMs, out var endedAtMs) ||
            JsonRead.ReadLong(element, "gap_ms") is not { } gapMs || gapMs <= 0 ||
            ReadConfidence(element, "peak_confidence") is not { } peakConfidence ||
            ReadConfidence(element, "last_confidence") is not { } lastConfidence ||
            JsonRead.ReadLong(element, "peak_co_occurrences") is not { } peakCoOccurrences || peakCoOccurrences < 0 ||
            JsonRead.ReadLong(element, "last_co_occurrences") is not { } lastCoOccurrences || lastCoOccurrences < 0 ||
            !TryReadTraceId(element, "sample_trace_id", out var sampleTraceId) ||
            !TryReadTraceId(element, "source_sample_trace_id", out var sourceSampleTraceId))
            return false;

        episode = new ParsedEpisode(id, pairKey, source, target, startedAtMs, lastSeenMs, endedAtMs, gapMs,
            peakConfidence, peakCoOccurrences, lastConfidence, lastCoOccurrences, ReadMedianLag(element),
            sampleTraceId, sourceSampleTraceId);
        return true;
    }

    private static EpisodeSide? ReadSide(JsonElement episode, string name)
    {
        if (!episode.TryGetProperty(name, out var side) ||
            side.ValueKind != JsonValueKind.Object ||
            JsonRead.ReadString(side, "finding_type") is not { } findingType ||
            Array.IndexOf(FindingLabels.Types, findingType) < 0 ||
            JsonRead.ReadString(side, "service") is not { Length: > 0 } service ||
            JsonRead.ReadString(side, "template") is not { } template)
            return null;
        return new EpisodeSide(findingType, service, template, JsonRead.ReadString(side, "grouping_key"),
            JsonRead.ReadString(side, "grouping_value"));
    }

    // Absent or null while the episode runs, otherwise no earlier than its start.
    private static bool TryReadEnd(JsonElement element, long startedAtMs, out long? endedAtMs)
    {
        endedAtMs = null;
        if (!element.TryGetProperty("ended_at_ms", out var value) || value.ValueKind == JsonValueKind.Null)
            return true;
        endedAtMs = JsonRead.ReadEpochMs(element, "ended_at_ms");
        return endedAtMs >= startedAtMs;
    }

    private static double? ReadConfidence(JsonElement element, string name)
    {
        return element.TryGetProperty(name, out var value) &&
               value.ValueKind == JsonValueKind.Number &&
               value.TryGetDouble(out var confidence) &&
               confidence is >= 0 and <= 1
            ? confidence
            : null;
    }

    // The lag is the daemon's estimate and nothing keys on it: an unreadable one is unknown.
    private static double? ReadMedianLag(JsonElement element)
    {
        return element.TryGetProperty("median_lag_ms", out var value) &&
               value.ValueKind == JsonValueKind.Number &&
               value.TryGetDouble(out var lag) &&
               lag >= 0
            ? lag
            : null;
    }

    private static bool TryReadTraceId(JsonElement element, string name, out string? traceId)
    {
        traceId = JsonRead.ReadString(element, name);
        return traceId is null or { Length: <= MaxTraceIdLength };
    }

    // The daemon prints both ids as SHA-256 prefixes in lowercase hex.
    private static bool IsHexId(string value)
    {
        return value.Length == 32 && value.All(char.IsAsciiHexDigitLower);
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: class `PerfSentinelHub.Tests.CorrelationParserTests`
Expected: PASS, 27 tests.

- [ ] **Step 6: Commit**

```bash
dotnet format PerfSentinelHub.sln && make format && git add PerfSentinelHub/Collection/CorrelationParser.cs PerfSentinelHub.Tests/EpisodeJson.cs PerfSentinelHub.Tests/CorrelationParserTests.cs && git commit -S -m "feat(import): parse a pushed batch of correlation episodes"
```

---

### Task 2: Store episodes under an order-free merge rule

**Files:**
- Modify: `PerfSentinelHub/Storage/Schema.cs` (add `V11` after `V10`)
- Modify: `PerfSentinelHub/Storage/HubDatabase.cs` (`InitializeAsync` migration text and version row, `PurgeChunkAsync`)
- Create: `PerfSentinelHub/Storage/HubDatabase.Correlations.cs`
- Test: `PerfSentinelHub.Tests/CorrelationStorageTests.cs` (create)
- Modify: `PerfSentinelHub.Tests/StorageTests.cs` (table list at line ~33, version string at line ~55)
- Modify: `PerfSentinelHub.Tests/WorkerAndRetentionTests.cs` (one purge test)

**Interfaces:**
- Consumes: `ParsedEpisode`, `EpisodeSide` (Task 1), `RecordImportAsync`, `WriteGateWait`, `_writeGate`, `SourceIdParameter`, `ObservedAtParameter` (`HubDatabase.cs`), `EpisodeJson` (Task 1).
- Produces: `Schema.V11`, table `correlation_episodes`, `public Task<bool> HubDatabase.TryUpsertEpisodesAsync(string sourceId, IReadOnlyList<ParsedEpisode> episodes, long observedAtMs, CancellationToken cancellationToken)`, false when the write gate stayed busy for 5 s.

- [ ] **Step 1: Write the failing tests**

Create `PerfSentinelHub.Tests/CorrelationStorageTests.cs`:

```csharp
using System.Text.Json.Nodes;
using Microsoft.Extensions.Options;
using PerfSentinelHub.Configuration;
using PerfSentinelHub.Storage;

namespace PerfSentinelHub.Tests;

public sealed class CorrelationStorageTests : IAsyncLifetime
{
    private const long ObservedAt = 1_800_000_500_000;

    private readonly string _path = Path.Combine(
        Path.GetTempPath(),
        $"perf-sentinel-hub-episodes-{Guid.NewGuid():N}.db");

    private HubDatabase _database = null!;

    public async ValueTask InitializeAsync()
    {
        _database = new HubDatabase(Options.Create(new HubOptions { DatabasePath = _path }), TimeProvider.System);
        await _database.InitializeAsync(TestContext.Current.CancellationToken);
    }

    public ValueTask DisposeAsync()
    {
        TestPool.ClearFor(_path);
        File.Delete(_path);
        File.Delete($"{_path}-shm");
        File.Delete($"{_path}-wal");
        return ValueTask.CompletedTask;
    }

    private async Task StoreAsync(string sourceId, params JsonObject[] copies)
    {
        // One call per copy, so a test decides the order in which they land.
        foreach (var copy in copies)
        {
            Assert.True(await _database.TryUpsertEpisodesAsync(
                sourceId, EpisodeJson.Parsed(copy), ObservedAt, TestContext.Current.CancellationToken));
        }
    }

    // One line per row, so a failure prints every merged column at once.
    private async Task<string> RowAsync(string id, string sourceId = "daemon-a")
    {
        return await ScalarAsync(
            """
            SELECT started_at_ms || ' ' || last_seen_ms || ' ' || IFNULL(ended_at_ms, '-') || ' ' ||
                   peak_confidence || ' ' || peak_co_occurrences || ' ' || last_confidence || ' ' ||
                   last_co_occurrences || ' ' || IFNULL(sample_trace_id, '-')
            FROM correlation_episodes WHERE id = $id AND source_id = $source_id;
            """, id, sourceId);
    }

    private async Task<string> ScalarAsync(string sql, string id = "", string sourceId = "daemon-a")
    {
        var cancellationToken = TestContext.Current.CancellationToken;
        await using var connection = await _database.OpenConnectionAsync(cancellationToken);
        await using var command = connection.CreateCommand();
        command.CommandText = sql;
        command.Parameters.AddWithValue("$id", id);
        command.Parameters.AddWithValue("$source_id", sourceId);
        return Convert.ToString(await command.ExecuteScalarAsync(cancellationToken),
            System.Globalization.CultureInfo.InvariantCulture)!;
    }

    [Fact]
    public async Task A_pushed_episode_keeps_both_sides_and_records_the_import()
    {
        await StoreAsync("daemon-a", EpisodeJson.Episode(EpisodeJson.Id(1)));

        Assert.Equal("1800000000000 1800000060000 - 0.92 41 0.88 37 4bf92f3577b34da6a3ce929d0e0e4736",
            await RowAsync(EpisodeJson.Id(1)));
        Assert.Equal("n_plus_one_sql order-svc pool_saturation payment-svc 1800000500000 1800000500000",
            await ScalarAsync(
                """
                SELECT source_type || ' ' || source_service || ' ' || target_type || ' ' || target_service || ' ' ||
                       first_seen_ms || ' ' || last_seen_hub_ms
                FROM correlation_episodes WHERE id = $id AND source_id = $source_id;
                """, EpisodeJson.Id(1)));
        Assert.Equal("1800000500000",
            await ScalarAsync("SELECT last_import_ms FROM source_imports WHERE source_id = $source_id;"));
        Assert.Equal("0", await ScalarAsync("SELECT COUNT(*) FROM source_state WHERE source_id = $source_id;"));
    }

    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public async Task Two_copies_converge_whatever_their_order(bool olderFirst)
    {
        var id = EpisodeJson.Id(2);
        var older = EpisodeJson.Episode(id);
        // A later round: a start precise to one round, rolling values that drifted, an end.
        var newer = EpisodeJson.Episode(id, 1_800_000_010_000, 1_800_000_120_000, 1_800_000_120_000);
        newer["peak_confidence"] = 0.8;
        newer["peak_co_occurrences"] = 30;
        newer["last_confidence"] = 0.7;
        newer["last_co_occurrences"] = 25;
        newer["sample_trace_id"] = "abababababababababababababababab";

        await StoreAsync("daemon-a", olderFirst ? [older, newer] : [newer, older]);

        Assert.Equal("1800000000000 1800000120000 1800000120000 0.92 41 0.7 25 abababababababababababababababab",
            await RowAsync(id));
    }

    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public async Task A_close_and_an_update_of_the_same_moment_converge_on_the_close(bool updateFirst)
    {
        var id = EpisodeJson.Id(3);
        var update = EpisodeJson.Episode(id);
        // Queued at the last co-occurrence of the update before it, its rolling values drifted since.
        var close = EpisodeJson.Episode(id, endedAtMs: 1_800_000_060_000);
        close["last_confidence"] = 0.8;
        close["last_co_occurrences"] = 30;

        await StoreAsync("daemon-a", updateFirst ? [update, close] : [close, update]);

        Assert.Equal("1800000000000 1800000060000 1800000060000 0.92 41 0.8 30 4bf92f3577b34da6a3ce929d0e0e4736",
            await RowAsync(id));
    }

    [Fact]
    public async Task Two_daemons_keep_a_copy_each()
    {
        var id = EpisodeJson.Id(4);
        var other = EpisodeJson.Episode(id);
        other["peak_confidence"] = 0.5;

        await StoreAsync("daemon-a", EpisodeJson.Episode(id));
        await StoreAsync("daemon-b", other);

        Assert.StartsWith("1800000000000 1800000060000 - 0.92 ", await RowAsync(id));
        Assert.StartsWith("1800000000000 1800000060000 - 0.5 ", await RowAsync(id, "daemon-b"));
    }
}
```

In `StorageTests.cs`, add `"correlation_episodes"` to the expected table list (sorted, after `"analysis_runs"`) and change the version string to `"1,2,3,4,5,6,7,8,9,10,11"`.

In `WorkerAndRetentionTests.cs`, after `Purge_removes_a_grouping_no_source_saw_since_the_cutoff`:

```csharp
    [Fact]
    public async Task Purge_drops_an_episode_last_seen_before_the_cutoff()
    {
        var cancellationToken = TestContext.Current.CancellationToken;
        var database = new HubDatabase(
            Options.Create(new HubOptions { DatabasePath = _databasePath }),
            TimeProvider.System);
        await database.InitializeAsync(cancellationToken);
        await database.TryUpsertEpisodesAsync("a", EpisodeJson.Parsed(
                EpisodeJson.Episode(EpisodeJson.Id(1), lastSeenMs: 1_800_000_060_000),
                EpisodeJson.Episode(EpisodeJson.Id(2), lastSeenMs: 1_800_000_960_000)),
            1_800_001_000_000, cancellationToken);

        await database.PurgeAsync(1_800_000_500_000, 0, 0, cancellationToken);

        await using var connection = await database.OpenConnectionAsync(cancellationToken);
        await using var read = connection.CreateCommand();
        read.CommandText = "SELECT group_concat(id, ',') FROM correlation_episodes;";
        Assert.Equal(EpisodeJson.Id(2), (string)(await read.ExecuteScalarAsync(cancellationToken))!);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: class `PerfSentinelHub.Tests.CorrelationStorageTests`
Expected: build error CS1061, `HubDatabase` has no `TryUpsertEpisodesAsync`.

- [ ] **Step 3: Add the schema**

In `Schema.cs`, after `V10`:

```csharp
    // The correlation episodes the daemons push: one row per period during which a pair of
    // findings kept following each other, cut by the daemon, which alone knows whether the
    // pair still co-occurs. Keyed on (id, source_id) like incidents, since two daemons can
    // see one pair. Each side sits in columns so a read filters on a service. started_at_ms,
    // last_seen_ms and ended_at_ms are the daemon's clock, first_seen_ms and
    // last_seen_hub_ms the Hub's. A rowid table because the purge deletes by rowid.
    internal const string V11 = """
                                CREATE TABLE IF NOT EXISTS correlation_episodes (
                                  id TEXT NOT NULL,
                                  source_id TEXT NOT NULL,
                                  pair_key TEXT NOT NULL,
                                  source_type TEXT NOT NULL,
                                  source_service TEXT NOT NULL,
                                  source_template TEXT NOT NULL,
                                  source_grouping TEXT,
                                  source_grouping_value TEXT,
                                  target_type TEXT NOT NULL,
                                  target_service TEXT NOT NULL,
                                  target_template TEXT NOT NULL,
                                  target_grouping TEXT,
                                  target_grouping_value TEXT,
                                  started_at_ms INTEGER NOT NULL,
                                  last_seen_ms INTEGER NOT NULL,
                                  ended_at_ms INTEGER,
                                  gap_ms INTEGER NOT NULL,
                                  peak_confidence REAL NOT NULL,
                                  peak_co_occurrences INTEGER NOT NULL,
                                  last_confidence REAL NOT NULL,
                                  last_co_occurrences INTEGER NOT NULL,
                                  median_lag_ms REAL,
                                  sample_trace_id TEXT,
                                  source_sample_trace_id TEXT,
                                  first_seen_ms INTEGER NOT NULL,
                                  last_seen_hub_ms INTEGER NOT NULL,
                                  PRIMARY KEY(id, source_id)
                                );
                                CREATE INDEX IF NOT EXISTS ix_episodes_started
                                  ON correlation_episodes(started_at_ms DESC);
                                CREATE INDEX IF NOT EXISTS ix_episodes_pair
                                  ON correlation_episodes(source_id, pair_key, started_at_ms);
                                CREATE INDEX IF NOT EXISTS ix_episodes_last_seen
                                  ON correlation_episodes(last_seen_ms);
                                """;
```

In `HubDatabase.cs` `InitializeAsync`, the migration text becomes:

```csharp
                migration.CommandText =
                    Schema.V1 + Schema.V2 + Schema.V3 + Schema.V4 + Schema.V5 + Schema.V6 + Schema.V7 + Schema.V8 +
                    Schema.V9 + Schema.V10 + Schema.V11;
```

and the version insert ends with:

```csharp
                                             (7, $applied_at_ms), (8, $applied_at_ms), (9, $applied_at_ms),
                                             (10, $applied_at_ms), (11, $applied_at_ms);
```

In `PurgeChunkAsync`, after the `incident_attributes` delete:

```sql
                                   DELETE FROM correlation_episodes WHERE rowid IN (
                                     SELECT rowid FROM correlation_episodes WHERE last_seen_ms < $cutoff LIMIT $chunk);
```

- [ ] **Step 4: Add the bounded upsert**

Create `PerfSentinelHub/Storage/HubDatabase.Correlations.cs`:

```csharp
using Microsoft.Data.Sqlite;
using PerfSentinelHub.Collection;

namespace PerfSentinelHub.Storage;

public sealed partial class HubDatabase
{
    // The copy carrying the newer state of one episode: the later last co-occurrence, or,
    // at the same one, the copy that closed it. A close is queued at the last_seen of the
    // update before it, with rolling values that drifted since, so without the second
    // clause two copies of one moment would keep whichever arrived first.
    private const string EpisodeNewer = """
                                        (excluded.last_seen_ms > correlation_episodes.last_seen_ms
                                         OR (excluded.last_seen_ms = correlation_episodes.last_seen_ms
                                             AND excluded.ended_at_ms IS NOT NULL
                                             AND correlation_episodes.ended_at_ms IS NULL))
                                        """;

    // The merge rule, so two copies converge in either order: the earliest start, the latest
    // last co-occurrence and the highest peaks, the rolling values of the newer copy, an end
    // kept once seen. The sides never change, they are what pair_key hashes. SQLite reads
    // every column of the SET from the row as it was, so the order of the lines is free.
    private const string EpisodeUpsert = $"""
                                          INSERT INTO correlation_episodes(
                                            id, source_id, pair_key, source_type, source_service, source_template,
                                            source_grouping, source_grouping_value, target_type, target_service,
                                            target_template, target_grouping, target_grouping_value,
                                            started_at_ms, last_seen_ms, ended_at_ms, gap_ms,
                                            peak_confidence, peak_co_occurrences, last_confidence, last_co_occurrences,
                                            median_lag_ms, sample_trace_id, source_sample_trace_id, first_seen_ms,
                                            last_seen_hub_ms)
                                          VALUES (
                                            $id, $source_id, $pair_key, $source_type, $source_service, $source_template,
                                            $source_grouping, $source_grouping_value, $target_type, $target_service,
                                            $target_template, $target_grouping, $target_grouping_value,
                                            $started_at_ms, $last_seen_ms, $ended_at_ms, $gap_ms,
                                            $peak_confidence, $peak_co_occurrences, $last_confidence,
                                            $last_co_occurrences, $median_lag_ms, $sample_trace_id,
                                            $source_sample_trace_id, $observed_at, $observed_at)
                                          ON CONFLICT(id, source_id) DO UPDATE SET
                                            started_at_ms = MIN(correlation_episodes.started_at_ms, excluded.started_at_ms),
                                            last_seen_ms = MAX(correlation_episodes.last_seen_ms, excluded.last_seen_ms),
                                            ended_at_ms = COALESCE(correlation_episodes.ended_at_ms, excluded.ended_at_ms),
                                            peak_confidence = MAX(correlation_episodes.peak_confidence, excluded.peak_confidence),
                                            peak_co_occurrences = MAX(correlation_episodes.peak_co_occurrences,
                                                                      excluded.peak_co_occurrences),
                                            last_confidence = CASE WHEN {EpisodeNewer}
                                              THEN excluded.last_confidence ELSE correlation_episodes.last_confidence END,
                                            last_co_occurrences = CASE WHEN {EpisodeNewer}
                                              THEN excluded.last_co_occurrences ELSE correlation_episodes.last_co_occurrences END,
                                            median_lag_ms = CASE WHEN {EpisodeNewer}
                                              THEN excluded.median_lag_ms ELSE correlation_episodes.median_lag_ms END,
                                            sample_trace_id = CASE WHEN {EpisodeNewer}
                                              THEN excluded.sample_trace_id ELSE correlation_episodes.sample_trace_id END,
                                            source_sample_trace_id = CASE WHEN {EpisodeNewer}
                                              THEN excluded.source_sample_trace_id
                                              ELSE correlation_episodes.source_sample_trace_id END,
                                            gap_ms = CASE WHEN {EpisodeNewer}
                                              THEN excluded.gap_ms ELSE correlation_episodes.gap_ms END,
                                            last_seen_hub_ms = excluded.last_seen_hub_ms;
                                          """;

    /// <summary>
    ///     Stores a daemon's pushed episodes under the merge rule. Waits for the write lock
    ///     as long as the two other imports do, then gives up: a 503 the daemon retries beats
    ///     a request held open behind a purge. Files the push in source_imports and never
    ///     touches source_state, which only the poll owns.
    /// </summary>
    public async Task<bool> TryUpsertEpisodesAsync(
        string sourceId,
        IReadOnlyList<ParsedEpisode> episodes,
        long observedAtMs,
        CancellationToken cancellationToken)
    {
        if (!await _writeGate.WaitAsync(WriteGateWait, cancellationToken))
            return false;
        try
        {
            await using var connection = await OpenConnectionAsync(cancellationToken);
            await using var transaction = connection.BeginTransaction(false);
            foreach (var episode in episodes)
            {
                await UpsertEpisodeAsync(connection, transaction, sourceId, episode, observedAtMs, cancellationToken);
            }

            await RecordImportAsync(connection, transaction, sourceId, observedAtMs, cancellationToken);
            await transaction.CommitAsync(cancellationToken);
            return true;
        }
        finally
        {
            _writeGate.Release();
        }
    }

    private static async Task UpsertEpisodeAsync(
        SqliteConnection connection,
        SqliteTransaction transaction,
        string sourceId,
        ParsedEpisode episode,
        long observedAtMs,
        CancellationToken cancellationToken)
    {
        await using var command = connection.CreateCommand();
        command.Transaction = transaction;
        command.CommandText = EpisodeUpsert;
        var parameters = command.Parameters;
        parameters.AddWithValue("$id", episode.Id);
        parameters.AddWithValue(SourceIdParameter, sourceId);
        parameters.AddWithValue("$pair_key", episode.PairKey);
        BindSide(parameters, "source", episode.Source);
        BindSide(parameters, "target", episode.Target);
        parameters.AddWithValue("$started_at_ms", episode.StartedAtMs);
        parameters.AddWithValue("$last_seen_ms", episode.LastSeenMs);
        parameters.AddWithValue("$ended_at_ms", (object?)episode.EndedAtMs ?? DBNull.Value);
        parameters.AddWithValue("$gap_ms", episode.GapMs);
        parameters.AddWithValue("$peak_confidence", episode.PeakConfidence);
        parameters.AddWithValue("$peak_co_occurrences", episode.PeakCoOccurrences);
        parameters.AddWithValue("$last_confidence", episode.LastConfidence);
        parameters.AddWithValue("$last_co_occurrences", episode.LastCoOccurrences);
        parameters.AddWithValue("$median_lag_ms", (object?)episode.MedianLagMs ?? DBNull.Value);
        parameters.AddWithValue("$sample_trace_id", (object?)episode.SampleTraceId ?? DBNull.Value);
        parameters.AddWithValue("$source_sample_trace_id", (object?)episode.SourceSampleTraceId ?? DBNull.Value);
        parameters.AddWithValue(ObservedAtParameter, observedAtMs);
        await command.ExecuteNonQueryAsync(cancellationToken);
    }

    private static void BindSide(SqliteParameterCollection parameters, string side, EpisodeSide value)
    {
        parameters.AddWithValue("$" + side + "_type", value.FindingType);
        parameters.AddWithValue("$" + side + "_service", value.Service);
        parameters.AddWithValue("$" + side + "_template", value.Template);
        parameters.AddWithValue("$" + side + "_grouping", (object?)value.GroupingKey ?? DBNull.Value);
        parameters.AddWithValue("$" + side + "_grouping_value", (object?)value.GroupingValue ?? DBNull.Value);
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: classes `PerfSentinelHub.Tests.CorrelationStorageTests`, `PerfSentinelHub.Tests.StorageTests`, `PerfSentinelHub.Tests.WorkerAndRetentionTests`
Expected: PASS for all three.

- [ ] **Step 6: Commit**

```bash
dotnet format PerfSentinelHub.sln && make format && git add PerfSentinelHub/Storage/Schema.cs PerfSentinelHub/Storage/HubDatabase.cs PerfSentinelHub/Storage/HubDatabase.Correlations.cs PerfSentinelHub.Tests/CorrelationStorageTests.cs PerfSentinelHub.Tests/StorageTests.cs PerfSentinelHub.Tests/WorkerAndRetentionTests.cs && git commit -S -m "feat(storage): keep pushed correlation episodes under an order-free merge"
```

---

### Task 3: List episodes with the overlap filter, the stale rule and access scoping

**Files:**
- Modify: `PerfSentinelHub/Api/ApiModels.cs` (after `IncidentQuery`)
- Modify: `PerfSentinelHub/Storage/StorageModels.cs` (after `StoredIncident`)
- Modify: `PerfSentinelHub/Storage/HubDatabase.Correlations.cs`
- Test: `PerfSentinelHub.Tests/CorrelationListTests.cs` (create)

**Interfaces:**
- Consumes: `TryUpsertEpisodesAsync` (Task 2), `Access(...)`, `AccessPatternsCte`, `BindAccess` (`HubDatabase.Access.cs`), `JsonArray(IReadOnlyList<string>)` (`HubDatabase.Incidents.cs`), `ServiceParameter`.
- Produces:
  - `public sealed record CorrelationQuery(string? Service, string? PairKey, IReadOnlyList<string>? SourceIds, long? FromMs, long? ToMs, bool Active, long NowMs, int Offset, int Limit, AccessScope? Access = null);`
  - `public sealed record StoredEpisode(string Id, string SourceId, string PairKey, EpisodeSide Source, EpisodeSide Target, long StartedAtMs, long LastSeenMs, long? EndedAtMs, string? EndedBy, long GapMs, double PeakConfidence, long PeakCoOccurrences, double LastConfidence, long LastCoOccurrences, double? MedianLagMs, string? SampleTraceId, string? SourceSampleTraceId, long FirstSeenMs, long LastSeenHubMs) { public string? SourceName { get; init; } }`, `EndedBy` being `"stale"` or null.
  - `public Task<IReadOnlyList<StoredEpisode>> HubDatabase.ListEpisodesAsync(CorrelationQuery query, CancellationToken cancellationToken)`, newest `started_at_ms` first.

- [ ] **Step 1: Write the failing tests**

Create `PerfSentinelHub.Tests/CorrelationListTests.cs`:

```csharp
using Microsoft.Extensions.Options;
using PerfSentinelHub.Api;
using PerfSentinelHub.Configuration;
using PerfSentinelHub.Storage;

namespace PerfSentinelHub.Tests;

public sealed class CorrelationListTests : IAsyncLifetime
{
    private const long Now = 1_800_010_000_000;

    // Running, closed by the daemon, and silent past its gap plus 15 minutes.
    private static readonly string Running = EpisodeJson.Id(1);
    private static readonly string Closed = EpisodeJson.Id(2);
    private static readonly string Silent = EpisodeJson.Id(3);

    private readonly string _path = Path.Combine(
        Path.GetTempPath(),
        $"perf-sentinel-hub-episodes-list-{Guid.NewGuid():N}.db");

    private HubDatabase _database = null!;

    public async ValueTask InitializeAsync()
    {
        var cancellationToken = TestContext.Current.CancellationToken;
        _database = new HubDatabase(Options.Create(new HubOptions { DatabasePath = _path }), TimeProvider.System);
        await _database.InitializeAsync(cancellationToken);
        await _database.TryUpsertEpisodesAsync("daemon-a", EpisodeJson.Parsed(
                EpisodeJson.Episode(Running, Now - 600_000, Now - 60_000, null, EpisodeJson.Id(0xa1)),
                EpisodeJson.Episode(Closed, Now - 7_200_000, Now - 6_600_000, Now - 6_600_000, EpisodeJson.Id(0xa2),
                    "gateway-svc", "order-svc"),
                EpisodeJson.Episode(Silent, Now - 5_000_000, Now - 3_000_000, null, EpisodeJson.Id(0xa3),
                    "cart-svc", "stock-svc")),
            Now, cancellationToken);
    }

    public ValueTask DisposeAsync()
    {
        TestPool.ClearFor(_path);
        File.Delete(_path);
        File.Delete($"{_path}-shm");
        File.Delete($"{_path}-wal");
        return ValueTask.CompletedTask;
    }

    private static CorrelationQuery All()
    {
        return new CorrelationQuery(null, null, null, null, null, false, Now, 0, 200);
    }

    private Task<IReadOnlyList<StoredEpisode>> ListAsync(CorrelationQuery query)
    {
        return _database.ListEpisodesAsync(query, TestContext.Current.CancellationToken);
    }

    private static string Ids(IEnumerable<StoredEpisode> rows)
    {
        return string.Join(',', rows.Select(row => row.Id[^1..]));
    }

    [Fact]
    public async Task Episodes_come_newest_first_with_a_silent_one_served_as_ended()
    {
        var rows = await ListAsync(All());

        Assert.Equal("1,3,2", Ids(rows));
        Assert.Null(rows[0].EndedAtMs);
        Assert.Null(rows[0].EndedBy);
        Assert.Equal(Now - 3_000_000, rows[1].EndedAtMs);
        Assert.Equal("stale", rows[1].EndedBy);
        Assert.Equal(Now - 6_600_000, rows[2].EndedAtMs);
        Assert.Null(rows[2].EndedBy);
        Assert.Equal("order-svc", rows[0].Source.Service);
        Assert.Equal("pool_saturation", rows[0].Target.FindingType);
        Assert.Equal(Now, rows[0].FirstSeenMs);
    }

    [Fact]
    public async Task Active_keeps_the_running_episodes_and_not_a_silent_one()
    {
        Assert.Equal("1", Ids(await ListAsync(All() with { Active = true })));
    }

    [Fact]
    public async Task A_late_close_still_lands_on_an_episode_served_as_stale()
    {
        var cancellationToken = TestContext.Current.CancellationToken;
        await using (var connection = await _database.OpenConnectionAsync(cancellationToken))
        await using (var command = connection.CreateCommand())
        {
            // The stale rule is a read-time verdict: the row itself holds no end.
            command.CommandText = "SELECT ended_at_ms IS NULL FROM correlation_episodes WHERE id = $id;";
            command.Parameters.AddWithValue("$id", Silent);
            Assert.Equal(1L, await command.ExecuteScalarAsync(cancellationToken));
        }

        await _database.TryUpsertEpisodesAsync("daemon-a", EpisodeJson.Parsed(
                EpisodeJson.Episode(Silent, Now - 5_000_000, Now - 3_000_000, Now - 3_000_000, EpisodeJson.Id(0xa3),
                    "cart-svc", "stock-svc")),
            Now + 1, cancellationToken);

        var row = Assert.Single(await ListAsync(All() with { PairKey = EpisodeJson.Id(0xa3) }));
        Assert.Null(row.EndedBy);
        Assert.Equal(Now - 3_000_000, row.EndedAtMs);
    }

    [Fact]
    public async Task A_service_matches_either_side()
    {
        Assert.Equal("1,2", Ids(await ListAsync(All() with { Service = "order-svc" })));
    }

    [Theory]
    [InlineData(-6_600_000L, -6_000_000L, "2")]
    [InlineData(-100_000L, 0L, "1")]
    [InlineData(-3_000_000L, -2_000_000L, "3")]
    [InlineData(-600_000L, -600_000L, "1")]
    [InlineData(-6_500_000L, -5_100_000L, "")]
    public async Task From_and_to_keep_the_episodes_overlapping_the_span(long from, long to, string expected)
    {
        // In order: an end exactly at from, a running episode, a served end exactly at from,
        // a start exactly at to, and a span between two episodes that touches neither.
        Assert.Equal(expected, Ids(await ListAsync(All() with { FromMs = Now + from, ToMs = Now + to })));
    }

    [Fact]
    public async Task Pages_and_the_closed_filters_narrow_the_listing()
    {
        Assert.Equal("3", Ids(await ListAsync(All() with { Offset = 1, Limit = 1 })));
        Assert.Equal("2", Ids(await ListAsync(All() with { PairKey = EpisodeJson.Id(0xa2) })));
        Assert.Equal("", Ids(await ListAsync(All() with { SourceIds = ["daemon-b"] })));
        Assert.Equal("1,3,2", Ids(await ListAsync(All() with { SourceIds = ["daemon-a"] })));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: class `PerfSentinelHub.Tests.CorrelationListTests`
Expected: build error CS0246, `CorrelationQuery` and `StoredEpisode` could not be found.

- [ ] **Step 3: Add the two records**

In `ApiModels.cs`, after `IncidentQuery`:

```csharp
// SourceIds is the resolved scope, as on IncidentQuery. FromMs and ToMs keep the episodes
// whose activity overlaps the span, bounds included, a running one reaching every later
// moment. NowMs dates the stale rule. Access null reads every episode, as with access off.
public sealed record CorrelationQuery(
    string? Service,
    string? PairKey,
    IReadOnlyList<string>? SourceIds,
    long? FromMs,
    long? ToMs,
    bool Active,
    long NowMs,
    int Offset,
    int Limit,
    AccessScope? Access = null);
```

In `StorageModels.cs`, after `StoredIncident`, with `using PerfSentinelHub.Collection;` at the top of the file:

```csharp
/// <summary>
///     One daemon's copy of a correlation episode as a read serves it. EndedAtMs is the
///     served end: the daemon's, or the last co-occurrence of an episode silent past its
///     gap plus 15 minutes, which is the only case EndedBy is set, to `stale`. SourceName
///     is the configured name, filled by the route, since storage holds ids only.
/// </summary>
public sealed record StoredEpisode(
    string Id,
    string SourceId,
    string PairKey,
    EpisodeSide Source,
    EpisodeSide Target,
    long StartedAtMs,
    long LastSeenMs,
    long? EndedAtMs,
    string? EndedBy,
    long GapMs,
    double PeakConfidence,
    long PeakCoOccurrences,
    double LastConfidence,
    long LastCoOccurrences,
    double? MedianLagMs,
    string? SampleTraceId,
    string? SourceSampleTraceId,
    long FirstSeenMs,
    long LastSeenHubMs)
{
    public string? SourceName { get; init; }
}
```

- [ ] **Step 4: Add the read**

Append to `HubDatabase.Correlations.cs`, with `using PerfSentinelHub.Api;` added at the top:

```csharp
    // What an open episode is served as once its daemon has said nothing of it for its gap
    // plus 15 minutes, most likely a daemon stopped before it could close it: ended at its
    // last co-occurrence. The row keeps its NULL, so a late close still lands.
    private const string EpisodeServedEnd = """
                                            CASE WHEN ce.ended_at_ms IS NOT NULL THEN ce.ended_at_ms
                                                 WHEN $now - ce.last_seen_ms > ce.gap_ms + 900000 THEN ce.last_seen_ms END
                                            """;

    private const string EpisodeListWhere = $"""
                                             SELECT ce.id, ce.source_id, ce.pair_key, ce.source_type, ce.source_service,
                                                    ce.source_template, ce.source_grouping, ce.source_grouping_value,
                                                    ce.target_type, ce.target_service, ce.target_template,
                                                    ce.target_grouping, ce.target_grouping_value,
                                                    ce.started_at_ms, ce.last_seen_ms, {EpisodeServedEnd},
                                                    ce.ended_at_ms IS NULL, ce.gap_ms, ce.peak_confidence,
                                                    ce.peak_co_occurrences, ce.last_confidence, ce.last_co_occurrences,
                                                    ce.median_lag_ms, ce.sample_trace_id, ce.source_sample_trace_id,
                                                    ce.first_seen_ms, ce.last_seen_hub_ms
                                             FROM correlation_episodes AS ce
                                             WHERE ($source_ids IS NULL OR ce.source_id IN (SELECT value FROM json_each($source_ids)))
                                               AND ($service IS NULL OR ce.source_service = $service OR ce.target_service = $service)
                                               AND ($pair_key IS NULL OR ce.pair_key = $pair_key)
                                               AND ($to IS NULL OR ce.started_at_ms <= $to)
                                               AND ($from IS NULL OR ({EpisodeServedEnd}) IS NULL OR ({EpisodeServedEnd}) >= $from)
                                               AND ($active = 0 OR ({EpisodeServedEnd}) IS NULL)
                                             """;

    private const string EpisodeListPage = """
                                           ORDER BY ce.started_at_ms DESC, ce.id ASC, ce.source_id ASC
                                           LIMIT $limit OFFSET $offset;
                                           """;

    private const string EpisodeList = $"{EpisodeListWhere}\n{EpisodeListPage}";

    // An episode names both services and both templates, so a scope reads it only where it
    // reads each side at that source. It carries no attribute, so a pattern keyed on one
    // never reads it: the subquery is empty and the value it compares is NULL.
    private const string NoEpisodeAttribute = "SELECT 1 WHERE 0";

    private static readonly string EpisodeAccess =
        Access("ce.source_id", "ce.source_service", NoEpisodeAttribute, "NULL") + " AND " +
        Access("ce.source_id", "ce.target_service", NoEpisodeAttribute, "NULL");

    // Declared after EpisodeAccess: static fields initialize in order.
    private static readonly string ScopedEpisodeList =
        $"WITH {AccessPatternsCte}\n{EpisodeListWhere} AND {EpisodeAccess}\n{EpisodeListPage}";

    /// <summary>
    ///     Newest start first, with the stale rule applied as the rows are read and the
    ///     caller's scope applied before the page, so pages are slices of what they read.
    /// </summary>
    public async Task<IReadOnlyList<StoredEpisode>> ListEpisodesAsync(
        CorrelationQuery query,
        CancellationToken cancellationToken)
    {
        await using var connection = await OpenConnectionAsync(cancellationToken);
        await using var command = connection.CreateCommand();
        command.CommandText = query.Access is null ? EpisodeList : ScopedEpisodeList;
        BindAccess(command, query.Access);
        var parameters = command.Parameters;
        parameters.AddWithValue("$source_ids", query.SourceIds is null ? DBNull.Value : JsonArray(query.SourceIds));
        parameters.AddWithValue(ServiceParameter, (object?)query.Service ?? DBNull.Value);
        parameters.AddWithValue("$pair_key", (object?)query.PairKey ?? DBNull.Value);
        parameters.AddWithValue("$from", (object?)query.FromMs ?? DBNull.Value);
        parameters.AddWithValue("$to", (object?)query.ToMs ?? DBNull.Value);
        parameters.AddWithValue("$active", query.Active ? 1 : 0);
        parameters.AddWithValue("$now", query.NowMs);
        parameters.AddWithValue("$limit", query.Limit);
        parameters.AddWithValue("$offset", query.Offset);

        var rows = new List<StoredEpisode>();
        await using var reader = await command.ExecuteReaderAsync(cancellationToken);
        while (await reader.ReadAsync(cancellationToken))
        {
            rows.Add(ReadEpisode(reader));
        }

        return rows;
    }

    private static StoredEpisode ReadEpisode(SqliteDataReader reader)
    {
        long? end = reader.IsDBNull(15) ? null : reader.GetInt64(15);
        // A served end on a row that holds none is the stale rule's.
        var endedBy = end is not null && reader.GetBoolean(16) ? "stale" : null;
        return new StoredEpisode(
            reader.GetString(0),
            reader.GetString(1),
            reader.GetString(2),
            new EpisodeSide(reader.GetString(3), reader.GetString(4), reader.GetString(5), OptionalText(reader, 6),
                OptionalText(reader, 7)),
            new EpisodeSide(reader.GetString(8), reader.GetString(9), reader.GetString(10), OptionalText(reader, 11),
                OptionalText(reader, 12)),
            reader.GetInt64(13),
            reader.GetInt64(14),
            end,
            endedBy,
            reader.GetInt64(17),
            reader.GetDouble(18),
            reader.GetInt64(19),
            reader.GetDouble(20),
            reader.GetInt64(21),
            reader.IsDBNull(22) ? null : reader.GetDouble(22),
            OptionalText(reader, 23),
            OptionalText(reader, 24),
            reader.GetInt64(25),
            reader.GetInt64(26));
    }

    private static string? OptionalText(SqliteDataReader reader, int ordinal)
    {
        return reader.IsDBNull(ordinal) ? null : reader.GetString(ordinal);
    }
```

If the compiler reports that `HubDatabase` already has a member named `OptionalText`, use that one and drop this copy.

- [ ] **Step 5: Run the tests to verify they pass**

Run: class `PerfSentinelHub.Tests.CorrelationListTests`
Expected: PASS, 11 tests.

- [ ] **Step 6: Commit**

```bash
dotnet format PerfSentinelHub.sln && make format && git add PerfSentinelHub/Api/ApiModels.cs PerfSentinelHub/Storage/StorageModels.cs PerfSentinelHub/Storage/HubDatabase.Correlations.cs PerfSentinelHub.Tests/CorrelationListTests.cs && git commit -S -m "feat(storage): list correlation episodes with overlap, stale and scope rules"
```

---

### Task 4: Count refused correlation imports

**Files:**
- Modify: `PerfSentinelHub/Api/ImportMetrics.cs`
- Modify: `PerfSentinelHub/Program.cs:79-80`
- Modify: `PerfSentinelHub/Api/MetricsEndpoint.cs` (`MapMetrics`, `RenderAsync`, after the incident import family)
- Test: `PerfSentinelHub.Tests/MetricsEndpointTests.cs`

**Interfaces:**
- Consumes: `ImportMetrics`, `ImportGate`, `ImportRejection`.
- Produces: `public sealed class CorrelationImportMetrics : ImportMetrics` and `public sealed record CorrelationImportAdmission(ImportGate Gate, CorrelationImportMetrics Metrics)` with `public T Refuse<T>(ImportRejection reason, T result)`, both singletons.

- [ ] **Step 1: Write the failing test** (append to `MetricsEndpointTests`, after `Refused_incident_imports_are_published_by_reason_from_startup`)

```csharp
    [Fact]
    public async Task Refused_correlation_imports_are_published_by_reason_from_startup()
    {
        var body = await ScrapeAsync(TestContext.Current.CancellationToken);
        Assert.Contains("# TYPE perf_sentinel_hub_correlation_import_rejected_total counter", body,
            StringComparison.Ordinal);
        foreach (var reason in new[] { "bad_request", "unauthorized", "gate_full", "write_timeout", "too_large" })
        {
            Assert.Contains(
                $"perf_sentinel_hub_correlation_import_rejected_total{{reason=\"{reason}\"}} 0",
                body,
                StringComparison.Ordinal);
        }
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: class `PerfSentinelHub.Tests.MetricsEndpointTests`
Expected: FAIL on `Refused_correlation_imports_are_published_by_reason_from_startup`, the series is absent.

- [ ] **Step 3: Implement**

In `ImportMetrics.cs`, after `IncidentImportAdmission`:

```csharp
/// <summary>
///     The correlation import's refusals, on a series of their own for the reason the
///     incident import has one.
/// </summary>
public sealed class CorrelationImportMetrics : ImportMetrics;

/// <summary>The correlation import's admission: the shared gate, its own refusal count.</summary>
public sealed record CorrelationImportAdmission(ImportGate Gate, CorrelationImportMetrics Metrics)
{
    public T Refuse<T>(ImportRejection reason, T result)
    {
        Metrics.Rejected(reason);
        return result;
    }
}
```

In `Program.cs`, after `builder.Services.AddSingleton<IncidentImportAdmission>();`:

```csharp
builder.Services.AddSingleton<CorrelationImportMetrics>();
builder.Services.AddSingleton<CorrelationImportAdmission>();
```

In `MetricsEndpoint.cs`, add `CorrelationImportMetrics correlationImports` after `IncidentImportMetrics incidentImports` in the `MapGet` lambda and in `RenderAsync`, pass it through, and wrap both the lambda's `MapMetrics` method and `RenderAsync` in the pragma the other injected endpoints use:

```csharp
    // Every parameter is a collaborator the router injects, one per metric family source.
#pragma warning disable S107
```

and `#pragma warning restore S107` after each signature. After the incident import loop:

```csharp
        Counter(text, "perf_sentinel_hub_correlation_import_rejected_total",
            "Correlation imports the Hub refused, by reason, published from startup like the findings ones.");
        foreach (var (reason, count) in correlationImports.Snapshot())
            Line(text, "perf_sentinel_hub_correlation_import_rejected_total", $"reason=\"{reason}\"", count);
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: class `PerfSentinelHub.Tests.MetricsEndpointTests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
dotnet format PerfSentinelHub.sln && make format && git add PerfSentinelHub/Api/ImportMetrics.cs PerfSentinelHub/Program.cs PerfSentinelHub/Api/MetricsEndpoint.cs PerfSentinelHub.Tests/MetricsEndpointTests.cs && git commit -S -m "feat(metrics): count refused correlation imports by reason"
```

---

### Task 5: The correlation import route

**Files:**
- Modify: `PerfSentinelHub/Api/ApiEndpoints.cs` (constants at the top, route map, `ImportIncidentsAsync`, new `ReadImportAsync` and `ImportCorrelationsAsync`, event 1303 beside 1302)
- Test: `PerfSentinelHub.Tests/CorrelationImportApiTests.cs` (create)

**Interfaces:**
- Consumes: `CorrelationParser.ParseImport` (Task 1), `HubDatabase.TryUpsertEpisodesAsync` (Task 2), `HubDatabase.ListEpisodesAsync` and `CorrelationQuery` (Task 3), `CorrelationImportAdmission` and `CorrelationImportMetrics` (Task 4), the existing `AdmitImport`, `ReadBodyAsync`, `ImportResponse`.
- Produces: `POST /api/import/correlations?source_id=<id>`, and `private static async Task<(T Import, IResult? Rejected)> ReadImportAsync<T>(HttpRequest request, int maxBytes, Func<ReadOnlyMemory<byte>, T> parse, Func<ImportRejection, IResult, IResult> refuse, CancellationToken cancellationToken) where T : class`, used by both the incident and correlation handlers.

- [ ] **Step 1: Write the failing tests**

Create `PerfSentinelHub.Tests/CorrelationImportApiTests.cs`:

```csharp
using System.Net;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.Extensions.DependencyInjection;
using PerfSentinelHub.Api;

namespace PerfSentinelHub.Tests;

public sealed class CorrelationImportApiTests(HubApplicationFactory factory) : IClassFixture<HubApplicationFactory>
{
    private const string ApiKey = "0123456789abcdef0123456789abcdef"; // gitleaks:allow -- synthetic test credential
    private readonly HttpClient _client = factory.CreateClient();

    private static HttpRequestMessage Request(string? apiKey, string body, string query = "source_id=test")
    {
        var request = new HttpRequestMessage(HttpMethod.Post, "/api/import/correlations?" + query)
        {
            Content = new StringContent(body, Encoding.UTF8, "application/json")
        };
        if (apiKey is not null)
            request.Headers.Add("X-API-Key", apiKey);
        return request;
    }

    private static string Body(params JsonObject[] episodes)
    {
        return EpisodeJson.Body(episodes);
    }

    private async Task<bool> StoredAsync(string id)
    {
        var rows = await factory.Database.ListEpisodesAsync(
            new CorrelationQuery(null, null, ["test"], null, null, false, 10_000, 0, 1000),
            TestContext.Current.CancellationToken);
        return rows.Any(row => row.Id == id);
    }

    [Theory]
    [InlineData(null)]
    [InlineData("wrong-key-wrong-key-wrong-key-wrong")]
    public async Task The_route_answers_to_the_source_import_key_alone(string? apiKey)
    {
        using var request = Request(apiKey, Body(EpisodeJson.Episode(EpisodeJson.Id(1))));
        using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

        Assert.Equal(HttpStatusCode.Unauthorized, response.StatusCode);
    }

    [Fact]
    public async Task An_unknown_source_is_refused_as_a_wrong_key_is()
    {
        using var request = Request(ApiKey, Body(EpisodeJson.Episode(EpisodeJson.Id(2))), "source_id=nobody");
        using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

        Assert.Equal(HttpStatusCode.Unauthorized, response.StatusCode);
    }

    [Fact]
    public async Task A_pushed_episode_is_stored_and_acknowledged_after_the_commit()
    {
        var cancellationToken = TestContext.Current.CancellationToken;
        using var request = Request(ApiKey, Body(EpisodeJson.Episode(EpisodeJson.Id(3))));
        using var response = await _client.SendAsync(request, cancellationToken);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var body = JsonDocument.Parse(await response.Content.ReadAsByteArrayAsync(cancellationToken));
        Assert.Equal(1, body.RootElement.GetProperty("accepted").GetInt32());
        Assert.Equal(0, body.RootElement.GetProperty("rejected").GetInt32());
        Assert.True(await StoredAsync(EpisodeJson.Id(3)));
    }

    [Fact]
    public async Task A_push_records_the_import_and_leaves_reachability_alone()
    {
        var cancellationToken = TestContext.Current.CancellationToken;
        using var request = Request(ApiKey, Body(EpisodeJson.Episode(EpisodeJson.Id(4))));
        using var response = await _client.SendAsync(request, cancellationToken);
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);

        await using var connection = await factory.Database.OpenConnectionAsync(cancellationToken);
        await using var command = connection.CreateCommand();
        command.CommandText = """
                              SELECT (SELECT last_import_ms FROM source_imports WHERE source_id = 'test'),
                                     (SELECT COUNT(*) FROM source_state WHERE source_id = 'test');
                              """;
        await using var reader = await command.ExecuteReaderAsync(cancellationToken);
        Assert.True(await reader.ReadAsync(cancellationToken));
        Assert.Equal(10_000, reader.GetInt64(0));
        Assert.Equal(0, reader.GetInt64(1));
    }

    [Fact]
    public async Task An_invalid_episode_is_refused_alone()
    {
        var cancellationToken = TestContext.Current.CancellationToken;
        var invalid = EpisodeJson.Episode("NOT-A-HEX-ID");
        using var request = Request(ApiKey, Body(EpisodeJson.Episode(EpisodeJson.Id(5)), invalid));
        using var response = await _client.SendAsync(request, cancellationToken);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var body = JsonDocument.Parse(await response.Content.ReadAsByteArrayAsync(cancellationToken));
        Assert.Equal(1, body.RootElement.GetProperty("accepted").GetInt32());
        Assert.Equal(1, body.RootElement.GetProperty("rejected").GetInt32());
    }

    [Fact]
    public async Task A_batch_of_only_invalid_episodes_is_a_bad_request()
    {
        using var request = Request(ApiKey, Body(EpisodeJson.Episode("NOT-A-HEX-ID")));
        using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    [Theory]
    [InlineData("{\"producer_version\":\"0.30.0\",\"episodes\":[]}")]
    [InlineData("{\"producer_version\":\"0.30.0\",\"producer_version\":\"0.29.0\",\"episodes\":[]}")]
    [InlineData("not json")]
    public async Task An_envelope_the_route_cannot_read_is_a_bad_request(string body)
    {
        using var request = Request(ApiKey, body);
        using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    [Fact]
    public async Task More_than_a_hundred_episodes_are_too_large()
    {
        var episodes = Enumerable.Range(100, 101).Select(index => EpisodeJson.Episode(EpisodeJson.Id(index))).ToArray();
        using var request = Request(ApiKey, Body(episodes));
        using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

        Assert.Equal(HttpStatusCode.RequestEntityTooLarge, response.StatusCode);
    }

    [Fact]
    public async Task A_body_over_one_mebibyte_is_too_large()
    {
        var episode = EpisodeJson.Episode(EpisodeJson.Id(6));
        episode["source"]!["template"] = new string('x', 1024 * 1024);
        using var request = Request(ApiKey, Body(episode));
        using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

        Assert.Equal(HttpStatusCode.RequestEntityTooLarge, response.StatusCode);
    }

    [Fact]
    public async Task A_full_import_gate_is_retryable_without_reading_the_body()
    {
        var gate = factory.Services.GetRequiredService<ImportGate>();
        var entered = Enumerable.Range(0, ImportGate.MaxImports).Count(_ => gate.TryEnter());
        try
        {
            using var request = Request(ApiKey, Body(EpisodeJson.Episode(EpisodeJson.Id(7))));
            using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

            Assert.Equal(HttpStatusCode.ServiceUnavailable, response.StatusCode);
            Assert.Equal("1", response.Headers.GetValues("Retry-After").Single());
        }
        finally
        {
            for (var slot = 0; slot < entered; slot++)
            {
                gate.Exit();
            }
        }
    }

    [Fact]
    public async Task A_second_query_parameter_is_a_bad_request()
    {
        using var request = Request(ApiKey, Body(EpisodeJson.Episode(EpisodeJson.Id(8))), "source_id=test&extra=1");
        using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    [Fact]
    public async Task A_refusal_counts_on_the_correlation_series()
    {
        var metrics = factory.Services.GetRequiredService<CorrelationImportMetrics>();
        var before = metrics.Snapshot().Single(entry => entry.Reason == "unauthorized").Count;

        using var request = Request(null, Body(EpisodeJson.Episode(EpisodeJson.Id(9))));
        using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

        Assert.Equal(HttpStatusCode.Unauthorized, response.StatusCode);
        Assert.Equal(before + 1, metrics.Snapshot().Single(entry => entry.Reason == "unauthorized").Count);
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: class `PerfSentinelHub.Tests.CorrelationImportApiTests`
Expected: FAIL, every request answers 404 Not Found (the gate test answers 404 too), and `A_refusal_counts_on_the_correlation_series` stays at its starting count.

- [ ] **Step 3: Share the body reading**

In `ApiEndpoints.cs`, add below `AdmitImport`:

```csharp
    /// <summary>
    ///     An admitted import's body, read under its cap and parsed, or the refusal it earned,
    ///     already counted: 413 past the cap or the batch limit, 400 for what the parser
    ///     refuses. The incident and correlation imports share it, their parsers differ.
    /// </summary>
    private static async Task<(T Import, IResult? Rejected)> ReadImportAsync<T>(
        HttpRequest request,
        int maxBytes,
        Func<ReadOnlyMemory<byte>, T> parse,
        Func<ImportRejection, IResult, IResult> refuse,
        CancellationToken cancellationToken)
        where T : class
    {
        var payload = await ReadBodyAsync(request, cancellationToken, maxBytes);
        if (payload is null)
            return (null!, refuse(ImportRejection.TooLarge,
                TypedResults.StatusCode(StatusCodes.Status413PayloadTooLarge)));
        try
        {
            return (parse(payload), null);
        }
        catch (ImportBatchTooLargeException)
        {
            return (null!, refuse(ImportRejection.TooLarge,
                TypedResults.StatusCode(StatusCodes.Status413PayloadTooLarge)));
        }
        catch (InvalidDataException)
        {
            return (null!, refuse(ImportRejection.BadRequest, TypedResults.BadRequest()));
        }
    }
```

In `ImportIncidentsAsync`, replace the block from `var payload = await ReadBodyAsync(...)` to the end of the `catch (InvalidDataException)` with:

```csharp
            var (import, rejected) = await ReadImportAsync<ParsedIncidentImport>(
                request, IncidentImportMaxBytes, IncidentParser.ParseImport, admission.Refuse, cancellationToken);
            if (rejected is not null)
                return rejected;
```

The rest of the handler (`import.Page.Incidents.Count == 0` onwards) is unchanged.

- [ ] **Step 4: Add the correlation handler and route**

Beside `IncidentImportMaxBytes`:

```csharp
    // A hundred episodes of two templates each: the daemon's own cap per batch.
    private const int CorrelationImportMaxBytes = 1024 * 1024;
```

In `MapHubApi`, after the incidents import mapping:

```csharp
        app.MapPost("/api/import/correlations", ImportCorrelationsAsync).AllowAnonymous();
```

Below `ImportIncidentsAsync`:

```csharp
    private static async Task<IResult> ImportCorrelationsAsync(
        HttpRequest request,
        HubDatabase database,
        CorrelationImportAdmission admission,
        IOptions<HubOptions> options,
        TimeProvider timeProvider,
        ILoggerFactory loggerFactory,
        CancellationToken cancellationToken)
    {
        if (AdmitImport(request, options, admission.Gate, admission.Refuse, out var source) is { } refused)
            return refused;
        try
        {
            var (import, rejected) = await ReadImportAsync<ParsedEpisodeImport>(
                request, CorrelationImportMaxBytes, CorrelationParser.ParseImport, admission.Refuse, cancellationToken);
            if (rejected is not null)
                return rejected;
            if (import.Episodes.Count == 0)
                return admission.Refuse(ImportRejection.BadRequest, TypedResults.BadRequest());
            if (!await database.TryUpsertEpisodesAsync(source.Id, import.Episodes,
                    timeProvider.GetUtcNow().ToUnixTimeMilliseconds(), cancellationToken))
            {
                request.HttpContext.Response.Headers.RetryAfter = "1";
                return admission.Refuse(ImportRejection.WriteTimeout,
                    TypedResults.StatusCode(StatusCodes.Status503ServiceUnavailable));
            }

            if (import.RejectedCount > 0)
                LogRejectedImportedEpisodes(loggerFactory.CreateLogger("ImportApi"), source.Id, import.RejectedCount);
            return TypedResults.Ok(new ImportResponse(import.Episodes.Count, import.RejectedCount));
        }
        finally
        {
            admission.Gate.Exit();
        }
    }
```

Beside event 1302:

```csharp
    [LoggerMessage(1303, LogLevel.Warning,
        "Source {SourceId} rejected {RejectedCount} imported correlation episodes.")]
    private static partial void LogRejectedImportedEpisodes(
        ILogger logger,
        string sourceId,
        int rejectedCount);
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: classes `PerfSentinelHub.Tests.CorrelationImportApiTests`, `PerfSentinelHub.Tests.IncidentImportApiTests`, `PerfSentinelHub.Tests.ImportApiTests`, `PerfSentinelHub.Tests.LoggerMessageLayoutTests`
Expected: PASS for all four (the second proves the shared reading kept the incident import intact).

- [ ] **Step 6: Commit**

```bash
dotnet format PerfSentinelHub.sln && make format && git add PerfSentinelHub/Api/ApiEndpoints.cs PerfSentinelHub.Tests/CorrelationImportApiTests.cs && git commit -S -m "feat(import): accept correlation episodes pushed to /api/import/correlations"
```

---

### Task 6: The correlation read route

**Files:**
- Create: `PerfSentinelHub/Api/ApiEndpoints.Correlations.cs`
- Modify: `PerfSentinelHub/Api/ApiEndpoints.cs` (route map)
- Modify: `PerfSentinelHub/Api/HubJsonContext.cs`
- Test: `PerfSentinelHub.Tests/CorrelationsApiTests.cs` (create)
- Modify: `PerfSentinelHub.Tests/AccessRoutesTests.cs` (credential list, 403 test, seed, one test)

**Interfaces:**
- Consumes: `ListEpisodesAsync`, `CorrelationQuery`, `StoredEpisode` (Task 3), `TryUpsertEpisodesAsync` (Task 2), the existing `NoFindingsRead`, `Narrowing`, `TryReadBounded`, `TryReadSourceScope`, `TryReadEpochMs`, `ReadOptional`, `HasValidUtf8`, `MaxFindingOffset`, `AccessPolicy.Scope`, `AccessPolicy.Sources`.
- Produces: `GET /api/correlations`, a bare JSON array of `StoredEpisode` in snake_case: `id`, `source_id`, `pair_key`, `source` and `target` (`finding_type`, `service`, `template`, `grouping_key`, `grouping_value`), `started_at_ms`, `last_seen_ms`, `ended_at_ms`, `ended_by` (`"stale"` or null), `gap_ms`, `peak_confidence`, `peak_co_occurrences`, `last_confidence`, `last_co_occurrences`, `median_lag_ms`, `sample_trace_id`, `source_sample_trace_id`, `first_seen_ms`, `last_seen_hub_ms`, `source_name`. Tasks 7 to 9, the lab and the engine plan's Grafana panels read these names.

- [ ] **Step 1: Write the failing tests**

Create `PerfSentinelHub.Tests/CorrelationsApiTests.cs`:

```csharp
using System.Net;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace PerfSentinelHub.Tests;

public sealed class CorrelationsApiTests(HubApplicationFactory factory) : IClassFixture<HubApplicationFactory>
{
    private readonly HttpClient _client = factory.CreateClient();

    private async Task SeedAsync(params JsonObject[] episodes)
    {
        Assert.True(await factory.Database.TryUpsertEpisodesAsync(
            "test", EpisodeJson.Parsed(episodes), 10_000, TestContext.Current.CancellationToken));
    }

    private async Task<JsonElement> GetAsync(string query)
    {
        var cancellationToken = TestContext.Current.CancellationToken;
        using var response = await _client.GetAsync("/api/correlations?" + query, cancellationToken);
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var document = JsonDocument.Parse(await response.Content.ReadAsByteArrayAsync(cancellationToken));
        return document.RootElement.Clone();
    }

    private static string Ids(JsonElement rows)
    {
        return string.Join(',', rows.EnumerateArray().Select(row => row.GetProperty("id").GetString()![^2..]));
    }

    [Fact]
    public async Task An_episode_is_served_in_snake_case_with_both_sides()
    {
        var pair = EpisodeJson.Id(0xb1);
        var grouped = EpisodeJson.Episode(EpisodeJson.Id(1), pairKey: pair);
        grouped["target"]!["grouping_key"] = "k8s.namespace.name";
        grouped["target"]!["grouping_value"] = "payments";
        await SeedAsync(grouped);

        var episode = Assert.Single((await GetAsync($"pair_key={pair}")).EnumerateArray());

        Assert.Equal(EpisodeJson.Id(1), episode.GetProperty("id").GetString());
        Assert.Equal("test", episode.GetProperty("source_id").GetString());
        Assert.Equal("Test", episode.GetProperty("source_name").GetString());
        Assert.Equal("n_plus_one_sql", episode.GetProperty("source").GetProperty("finding_type").GetString());
        Assert.Equal(JsonValueKind.Null, episode.GetProperty("source").GetProperty("grouping_value").ValueKind);
        Assert.Equal("payments", episode.GetProperty("target").GetProperty("grouping_value").GetString());
        Assert.Equal("payment-svc", episode.GetProperty("target").GetProperty("service").GetString());
        Assert.Equal(1_800_000_000_000, episode.GetProperty("started_at_ms").GetInt64());
        Assert.Equal(JsonValueKind.Null, episode.GetProperty("ended_at_ms").ValueKind);
        Assert.Equal(JsonValueKind.Null, episode.GetProperty("ended_by").ValueKind);
        Assert.Equal(0.92, episode.GetProperty("peak_confidence").GetDouble());
        Assert.Equal(1200.0, episode.GetProperty("median_lag_ms").GetDouble());
        Assert.Equal("00f067aa0ba902b74bf92f3577b34da6", episode.GetProperty("source_sample_trace_id").GetString());
        Assert.Equal(600_000, episode.GetProperty("gap_ms").GetInt64());
    }

    [Fact]
    public async Task Service_active_and_the_span_narrow_the_listing()
    {
        await SeedAsync(
            EpisodeJson.Episode(EpisodeJson.Id(0x11), 1_800_000_000_000, 1_800_000_060_000, 1_800_000_060_000,
                EpisodeJson.Id(0xc1), "api-a", "api-b"),
            EpisodeJson.Episode(EpisodeJson.Id(0x12), 1_800_000_500_000, 1_800_000_560_000, null,
                EpisodeJson.Id(0xc2), "api-c", "api-a"));

        Assert.Equal("12,11", Ids(await GetAsync("service=api-a")));
        Assert.Equal("12", Ids(await GetAsync("service=api-a&active=true")));
        Assert.Equal("11", Ids(await GetAsync("service=api-a&from=1800000000000&to=1800000100000")));
        Assert.Equal("12", Ids(await GetAsync("service=api-a&from=1800000400000")));
        Assert.Equal("11", Ids(await GetAsync("service=api-a&limit=1&offset=1")));
        Assert.Equal("12,11", Ids(await GetAsync("service=api-a&source_id=test&environment=test")));
    }

    [Fact]
    public async Task A_blank_filter_means_no_filter()
    {
        var pair = EpisodeJson.Id(0xb2);
        await SeedAsync(EpisodeJson.Episode(EpisodeJson.Id(0x21), pairKey: pair));

        // What Grafana sends for a variable set to All.
        Assert.Equal("21", Ids(await GetAsync(
            $"pair_key={pair}&service=&source_id=%20&environment=&from=&to=&active=")));
    }

    [Fact]
    public async Task The_listing_stops_at_200_unless_asked_for_up_to_1000()
    {
        var pair = EpisodeJson.Id(0xd1);
        for (var batch = 0; batch < 3; batch++)
        {
            await SeedAsync([
                .. Enumerable.Range(0x1000 + batch * 70, 70)
                    .Select(index => EpisodeJson.Episode(EpisodeJson.Id(index), pairKey: pair))
            ]);
        }

        Assert.Equal(200, (await GetAsync($"pair_key={pair}")).GetArrayLength());
        Assert.Equal(210, (await GetAsync($"pair_key={pair}&limit=1000")).GetArrayLength());
    }

    [Theory]
    [InlineData("source_id=nobody")]
    [InlineData("environment=nowhere")]
    [InlineData("from=2&to=1")]
    [InlineData("from=yesterday")]
    [InlineData("limit=0")]
    [InlineData("limit=1001")]
    [InlineData("offset=-1")]
    [InlineData("active=yes")]
    [InlineData("service=a&service=b")]
    public async Task A_query_the_route_cannot_read_is_a_bad_request(string query)
    {
        using var response = await _client.GetAsync("/api/correlations?" + query,
            TestContext.Current.CancellationToken);

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }
}
```

In `AccessRoutesTests.cs`:

- add `[InlineData("GET", "/api/correlations")]` to `Every_route_needs_a_credential`, after the incidents lines;
- in `A_caller_without_findings_read_anywhere_gets_403_on_both_findings_routes`, add `using var correlations = await client.GetAsync("/api/correlations", cancellationToken);` and `Assert.Equal(HttpStatusCode.Forbidden, correlations.StatusCode);`;
- at the end of `SeedAsync`, call `await SeedEpisodesAsync();`, and add beside `SeedIncidentAsync`:

```csharp
    // e1 is cart then cart at shop-d, e2 cart then invoice at shop-d, e3 cart then cart at lab-d.
    private async Task SeedEpisodesAsync()
    {
        var cancellationToken = TestContext.Current.CancellationToken;
        await Database.TryUpsertEpisodesAsync("shop-d", EpisodeJson.Parsed(
                EpisodeJson.Episode(EpisodeJson.Id(0xe1), 1_800_000_300_000, 1_800_000_360_000, null,
                    EpisodeJson.Id(0xa1), "cart", "cart"),
                EpisodeJson.Episode(EpisodeJson.Id(0xe2), 1_800_000_200_000, 1_800_000_260_000, null,
                    EpisodeJson.Id(0xa2), "cart", "invoice")),
            NowMs, cancellationToken);
        await Database.TryUpsertEpisodesAsync("lab-d", EpisodeJson.Parsed(
                EpisodeJson.Episode(EpisodeJson.Id(0xe3), 1_800_000_100_000, 1_800_000_160_000, null,
                    EpisodeJson.Id(0xa1), "cart", "cart")),
            NowMs, cancellationToken);
    }

    private async Task<string> EpisodesAsync(string user)
    {
        using var client = await SignedInAsync(user);
        return string.Join(',', (await FindingsAsync(client, "/api/correlations")).Select(episode =>
            episode.GetProperty("id").GetString()![^2..] + " " + episode.GetProperty("source_id").GetString()));
    }
```

and the test, in the incidents block:

```csharp
    [Fact]
    public async Task An_episode_is_read_only_where_both_its_services_are()
    {
        // svc reads cart anywhere, so not e2 whose target is invoice. ss reads cart at shop-d,
        // lab reads all of lab-d, and attr reads only a team attribute an episode never carries.
        Assert.Equal("e1 shop-d,e3 lab-d", await EpisodesAsync("svc"));
        Assert.Equal("e1 shop-d", await EpisodesAsync("ss"));
        Assert.Equal("e3 lab-d", await EpisodesAsync("lab"));
        Assert.Equal("", await EpisodesAsync("attr"));
        Assert.Equal("e1 shop-d,e2 shop-d,e3 lab-d", await EpisodesAsync("admin"));
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: classes `PerfSentinelHub.Tests.CorrelationsApiTests`, `PerfSentinelHub.Tests.AccessRoutesTests`
Expected: FAIL, `/api/correlations` answers 404 everywhere (the launcher's fallback serves no such route), so the OK assertions and the 401 and 403 assertions on that route fail.

- [ ] **Step 3: Implement the route**

Create `PerfSentinelHub/Api/ApiEndpoints.Correlations.cs`:

```csharp
using Microsoft.Extensions.Options;
using PerfSentinelHub.Configuration;
using PerfSentinelHub.Storage;

namespace PerfSentinelHub.Api;

public static partial class ApiEndpoints
{
    // The spec's own bounds, not Hub:MaxReadLimit: a screen reads a range of episodes
    // whole, and a thousand rows of a few hundred bytes each is one cheap answer.
    private const int CorrelationDefaultLimit = 200;
    private const int CorrelationMaxLimit = 1000;

    /// <summary>
    ///     The episodes the daemons pushed, scoped as the incidents are: findings.read, and
    ///     with access on only the episodes whose two services the caller reads at their
    ///     source. Nothing reads a daemon here, the Hub serves what it was pushed.
    /// </summary>
    private static async Task<IResult> GetCorrelationsAsync(
        HttpContext context,
        HubDatabase database,
        AccessPolicy access,
        IOptions<HubOptions> options,
        TimeProvider timeProvider,
        CancellationToken cancellationToken)
    {
        var scope = access.Scope(context, AccessPermissions.FindingsRead);
        if (scope.IsNothing)
            return NoFindingsRead();
        if (!TryParseCorrelationQuery(context.Request, access.Sources(context, options.Value.Sources),
                timeProvider.GetUtcNow().ToUnixTimeMilliseconds(), out var query))
            return TypedResults.BadRequest();
        var rows = await database.ListEpisodesAsync(query with { Access = Narrowing(scope) }, cancellationToken);
        // Storage holds ids, the configuration names them, as IncidentWriter does for incidents.
        IReadOnlyList<StoredEpisode> named =
        [
            .. rows.Select(row => row with
            {
                SourceName = options.Value.Sources.FirstOrDefault(source =>
                    string.Equals(source.Id, row.SourceId, StringComparison.Ordinal))?.Name
            })
        ];
        return TypedResults.Ok(named);
    }

    private static bool TryParseCorrelationQuery(
        HttpRequest request,
        IReadOnlyList<SourceOptions> sources,
        long nowMs,
        out CorrelationQuery query)
    {
        query = null!;
        if (!HasValidUtf8(request.QueryString.Value) || request.Query.Any(item => item.Value.Count != 1))
            return false;
        if (!TryReadBounded(request, "limit", CorrelationDefaultLimit, 1, CorrelationMaxLimit, out var limit) ||
            !TryReadBounded(request, "offset", 0, 0, MaxFindingOffset, out var offset) ||
            !TryReadSourceScope(request, sources, out var sourceIds) ||
            !TryReadEpochMs(request, "from", out var fromMs) ||
            !TryReadEpochMs(request, "to", out var toMs) ||
            fromMs > toMs)
            return false;

        // Blank reads as absent, as every filter of this route does through ReadOptional.
        var active = false;
        if (ReadOptional(request, "active") is { } rawActive && !bool.TryParse(rawActive, out active))
            return false;

        query = new CorrelationQuery(
            ReadOptional(request, "service"),
            ReadOptional(request, "pair_key"),
            sourceIds,
            fromMs,
            toMs,
            active,
            nowMs,
            offset,
            limit);
        return true;
    }
}
```

In `ApiEndpoints.cs` `MapHubApi`, after `app.MapPost("/api/incidents/refresh", RefreshIncidentsAsync);`:

```csharp
        app.MapGet("/api/correlations", GetCorrelationsAsync);
```

In `HubJsonContext.cs`, add `using PerfSentinelHub.Storage;` and, after the `SourceResponse` line:

```csharp
[JsonSerializable(typeof(IReadOnlyList<StoredEpisode>))]
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: classes `PerfSentinelHub.Tests.CorrelationsApiTests`, `PerfSentinelHub.Tests.AccessRoutesTests`, then the whole suite `./PerfSentinelHub.Tests/bin/Release/net10.0/PerfSentinelHub.Tests`
Expected: PASS everywhere, `AuthenticationTests`, `AccessOffCompatibilityTests` and `IncidentsApiTests` included.

- [ ] **Step 5: Commit**

```bash
dotnet format PerfSentinelHub.sln && make format && git add PerfSentinelHub/Api/ApiEndpoints.Correlations.cs PerfSentinelHub/Api/ApiEndpoints.cs PerfSentinelHub/Api/HubJsonContext.cs PerfSentinelHub.Tests/CorrelationsApiTests.cs PerfSentinelHub.Tests/AccessRoutesTests.cs && git commit -S -m "feat(api): serve correlation episodes on /api/correlations"
```

---

### Task 7: Launcher helpers for pairs, windows and strips

**Files:**
- Modify: `PerfSentinelHub/wwwroot/launcher.js` (`readHandoff` near line 1085, `screenOf` near line 1447, new functions beside `incidentHandoffHash`, the `global.PSL` export block)
- Test: `tests/launcher.test.js`

**Interfaces:**
- Consumes: `parseDur`, `dtHuman`, `dur` (launcher.js), the episode shape of Task 6.
- Produces on `PSL`: `CORRELATION_RANGES` (`['1h', '6h', '24h', '7d', '30d', '180d']`), `readCorrelationPair(hash)` (the `pair_key` of a `#/correlations?pair_key=…` hash, or `''`), `correlationPairs(episodes)` returning `[{key, source_id, pair_key, episodes, active, peak, last}]` (`key` is `source_id + ':' + pair_key`), `spansOverlap(aFrom, aTo, bFrom, bTo)`, `episodeHandoffHash(episode, nowMs)`, `pairTraceHash(episode)` (string or null), `readTraceHandoff(hash)` returning `{traceId, otherTraceId}` or null, `timelineMarks(spans, fromMs, toMs, width)` returning `[{x, w}]`, `lagText(ms)`, `episodeSpan(startMs, endMs)`. `readHandoff` gains `pairKey`. `screenOf('#/correlations', …)` is `'correlations'`.

- [ ] **Step 1: Write the failing tests**

In `tests/launcher.test.js`, change the `deepEqual` of `an incident's window travels to New analysis as a hash and back` to:

```js
    assert.deepEqual(PSL.readHandoff(hash, 9000),
        {fromMs: 1000, toMs: 5000, service: "shop svc", incidentId: "d650edad80ac5c2d99b8d1dde07100c2", pairKey: ""});
```

add to the `screenOf` test, next to line ~1004:

```js
    assert.equal(PSL.screenOf("#/correlations", false, false), "correlations");
```

and append:

```js
// ------------------------------------------------------------ correlations

const episode = (over) => Object.assign({
    source_id: "checkout-prod",
    pair_key: "a".repeat(32),
    started_at_ms: 1_000_000,
    last_seen_ms: 1_060_000,
    ended_at_ms: 1_060_000,
    peak_confidence: 0.8,
    median_lag_ms: 1200,
    source: {finding_type: "n_plus_one_sql", service: "order svc", template: "SELECT 1", grouping_key: null},
    target: {finding_type: "pool_saturation", service: "orders-svc", template: "SELECT 2", grouping_key: null},
    sample_trace_id: "b".repeat(32),
    source_sample_trace_id: "c".repeat(32)
}, over);

test("episodes fold into one row per pair and daemon, the most recently active first", () => {
    const pairs = PSL.correlationPairs([
        episode({started_at_ms: 3_000_000, last_seen_ms: 3_100_000, ended_at_ms: null, peak_confidence: 0.7}),
        episode({pair_key: "d".repeat(32), started_at_ms: 2_000_000, last_seen_ms: 2_050_000, ended_at_ms: 2_050_000}),
        episode({peak_confidence: 0.92}),
        episode({source_id: "orders-prod", started_at_ms: 500_000, last_seen_ms: 560_000, ended_at_ms: 560_000})
    ]);
    assert.deepEqual(pairs.map((pair) => [pair.source_id, pair.pair_key[0], pair.episodes.length, pair.active, pair.peak]),
        [["checkout-prod", "a", 2, true, 0.92], ["checkout-prod", "d", 1, false, 0.8], ["orders-prod", "a", 1, false, 0.8]]);
    assert.equal(pairs[0].last.started_at_ms, 3_000_000, "the last episode is the latest started");
    assert.equal(pairs[0].key, "checkout-prod:" + "a".repeat(32));
});

test("two spans share a moment on their bounds, and a running one reaches every later moment", () => {
    assert.equal(PSL.spansOverlap(0, 10, 10, 20), true, "an end on the other's start");
    assert.equal(PSL.spansOverlap(0, 10, 11, 20), false);
    assert.equal(PSL.spansOverlap(0, null, 1_000, 2_000), true, "still running");
    assert.equal(PSL.spansOverlap(5_000, null, 1_000, 2_000), false, "started after the span");
});

test("an episode's window travels to New analysis a minute wider on each side, with its pair", () => {
    const hash = PSL.episodeHandoffHash(episode({}), 9_000_000);
    assert.equal(hash, "#/new?from=940000&to=1120000&service=order%20svc&pair=" + "a".repeat(32));
    assert.deepEqual(PSL.readHandoff(hash, 9_000_000),
        {fromMs: 940000, toMs: 1120000, service: "order svc", incidentId: "", pairKey: "a".repeat(32)});
});

test("a running episode's window ends now, never in a future the form refuses", () => {
    assert.match(PSL.episodeHandoffHash(episode({ended_at_ms: null}), 1_500_000), /&to=1500000&/);
    assert.match(PSL.episodeHandoffHash(episode({}), 1_100_000), /&to=1100000&/, "a minute past the end is not yet");
});

test("the last pair's traces open New analysis on the first one, the other carried along", () => {
    assert.equal(PSL.pairTraceHash(episode({})), "#/new?trace=" + "c".repeat(32) + "&other=" + "b".repeat(32));
    assert.equal(PSL.pairTraceHash(episode({source_sample_trace_id: null})), "#/new?trace=" + "b".repeat(32));
    assert.equal(PSL.pairTraceHash(episode({source_sample_trace_id: null, sample_trace_id: null})), null);
    assert.deepEqual(PSL.readTraceHandoff("#/new?trace=x1&other=y2"), {traceId: "x1", otherTraceId: "y2"});
    assert.deepEqual(PSL.readTraceHandoff("#/new?trace=x1"), {traceId: "x1", otherTraceId: ""});
    assert.equal(PSL.readTraceHandoff("#/new?from=1&to=2&service=s"), null);
    assert.equal(PSL.readTraceHandoff("#/correlations?trace=x1"), null);
    assert.equal(PSL.readTraceHandoff(null), null);
});

test("a strip places each span on its range, clipped, and never thinner than three", () => {
    assert.deepEqual(PSL.timelineMarks(
        [{from: 250, to: 500}, {from: -100, to: 50}, {from: 900, to: null}, {from: 400, to: 401}], 0, 1000, 640),
    [{x: 160, w: 160}, {x: 0, w: 32}, {x: 576, w: 64}, {x: 256, w: 3}]);
});

test("a lag keeps its tenth of a second, which dur() would round away", () => {
    assert.equal(PSL.lagText(850), "850 ms");
    assert.equal(PSL.lagText(1200), "1.2 s");
    assert.equal(PSL.lagText(90_000), "1 m 30 s");
    assert.equal(PSL.lagText(null), "—");
});

test("an episode's span names its start whole and its end as a clock on the same day", () => {
    const at = (day, hour, minute) => new Date(2026, 9, day, hour, minute).getTime();
    assert.equal(PSL.episodeSpan(at(15, 11, 55), at(15, 12, 25)), "2026-10-15 11:55 → 12:25");
    assert.equal(PSL.episodeSpan(at(15, 23, 50), at(16, 0, 20)), "2026-10-15 23:50 → 2026-10-16 00:20");
});

test("a link to one pair carries its key in the correlations route", () => {
    assert.equal(PSL.readCorrelationPair("#/correlations?pair_key=" + "a".repeat(32)), "a".repeat(32));
    assert.equal(PSL.readCorrelationPair("#/correlations?pair_key=%20"), "", "blank is no filter");
    assert.equal(PSL.readCorrelationPair("#/correlations"), "");
    assert.equal(PSL.readCorrelationPair("#/incidents?pair_key=x"), "");
    assert.equal(PSL.readCorrelationPair(null), "");
    assert.equal(PSL.screenOf("#/correlations?pair_key=x", false, false), "correlations");
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `node --test tests/launcher.test.js`
Expected: FAIL, `PSL.correlationPairs is not a function` and the rest of the new helpers likewise, the `readHandoff` deepEqual missing `pairKey`, and `screenOf` answering `missing`.

- [ ] **Step 3: Implement**

In `launcher.js`, `readHandoff` returns the pair as well. Its JSDoc `@returns` becomes `{{fromMs: number, toMs: number, service: string, incidentId: string, pairKey: string} | null}` and its last line:

```js
        return {
            fromMs: fromMs,
            toMs: toMs,
            service: service,
            incidentId: params.get('incident') || '',
            pairKey: params.get('pair') || ''
        };
```

In `screenOf`, the last line becomes:

```js
        return ['new', 'recent', 'sources', 'incidents', 'correlations', 'ack'].indexOf(route) >= 0 ? route : 'missing';
```

After `readHandoff`, add:

```js
    /**
     * The correlations screen's ranges. History is the point of the screen, so they reach
     * past the week New analysis stops at, up to the 180 days the Hub keeps by default.
     * @type {string[]}
     */
    const CORRELATION_RANGES = ['1h', '6h', '24h', '7d', '30d', '180d'];

    /** How far past each end of an episode its analysis window reaches. */
    const EPISODE_MARGIN_MS = 60000;

    /**
     * The pair a `#/correlations?pair_key=…` link names, the route the Grafana table and an
     * incident's "All its episodes" link to, or '' for every pair. Blank is no filter.
     * @param {string | null | undefined} hash
     * @returns {string}
     */
    function readCorrelationPair(hash) {
        const text = String(hash || '');
        if (text.indexOf('#/correlations?') !== 0) return '';
        return (new URLSearchParams(text.slice('#/correlations?'.length)).get('pair_key') || '').trim();
    }

    /**
     * One row per pair and daemon, built from the episodes of one read, the pair whose last
     * episode was seen most recently first. A row keeps its episodes in the route's order,
     * newest start first, and names the latest started one as its last.
     * @param {Array<{source_id: string, pair_key: string, started_at_ms: number, last_seen_ms: number,
     *     ended_at_ms: number | null, peak_confidence: number}>} episodes
     * @returns {Array<{key: string, source_id: string, pair_key: string, episodes: any[], active: boolean,
     *     peak: number, last: any}>}
     */
    function correlationPairs(episodes) {
        /** @type {Map<string, {key: string, source_id: string, pair_key: string, episodes: any[], active: boolean, peak: number, last: any}>} */
        const pairs = new Map();
        episodes.forEach(function (episode) {
            const key = episode.source_id + ':' + episode.pair_key;
            const pair = pairs.get(key) || {
                key: key, source_id: episode.source_id, pair_key: episode.pair_key,
                episodes: [], active: false, peak: 0, last: episode
            };
            pair.episodes.push(episode);
            pair.active = pair.active || episode.ended_at_ms == null;
            pair.peak = Math.max(pair.peak, episode.peak_confidence);
            if (episode.started_at_ms > pair.last.started_at_ms) pair.last = episode;
            pairs.set(key, pair);
        });
        return Array.from(pairs.values()).sort(function (a, b) {
            return b.last.last_seen_ms - a.last.last_seen_ms || (a.key < b.key ? -1 : 1);
        });
    }

    /**
     * Whether two spans share a moment, bounds included. A null end is a span still running.
     * @param {number} aFrom
     * @param {number | null} aTo
     * @param {number} bFrom
     * @param {number | null} bTo
     * @returns {boolean}
     */
    function spansOverlap(aFrom, aTo, bFrom, bTo) {
        return aFrom <= (bTo == null ? Infinity : bTo) && bFrom <= (aTo == null ? Infinity : aTo);
    }

    /**
     * New analysis on an episode's window, a minute wider on each side and held at now, on
     * the service of the side that fires first, the way an incident hands over its own. The
     * pair rides along for the banner's wording.
     * @param {{started_at_ms: number, ended_at_ms: number | null, pair_key: string, source: {service: string}}} episode
     * @param {number} nowMs
     * @returns {string}
     */
    function episodeHandoffHash(episode, nowMs) {
        const end = episode.ended_at_ms == null ? nowMs : episode.ended_at_ms + EPISODE_MARGIN_MS;
        return '#/new?from=' + (episode.started_at_ms - EPISODE_MARGIN_MS)
            + '&to=' + Math.min(end, nowMs)
            + '&service=' + encodeURIComponent(episode.source.service)
            + '&pair=' + encodeURIComponent(episode.pair_key);
    }

    /**
     * New analysis on a trace of the episode's last co-occurrence: the first finding's, the
     * second's carried for the banner, or the second's alone. Null when the daemon kept neither.
     * @param {{sample_trace_id: string | null, source_sample_trace_id: string | null}} episode
     * @returns {string | null}
     */
    function pairTraceHash(episode) {
        const first = episode.source_sample_trace_id || episode.sample_trace_id;
        if (!first) return null;
        const other = episode.source_sample_trace_id && episode.sample_trace_id;
        return '#/new?trace=' + encodeURIComponent(first) + (other ? '&other=' + encodeURIComponent(other) : '');
    }

    /**
     * What a `#/new?trace=…&other=…` hash carries, or null for any other route.
     * @param {string | null | undefined} hash
     * @returns {{traceId: string, otherTraceId: string} | null}
     */
    function readTraceHandoff(hash) {
        const text = String(hash || '');
        if (text.indexOf('#/new?') !== 0) return null;
        const params = new URLSearchParams(text.slice('#/new?'.length));
        const traceId = (params.get('trace') || '').trim();
        return traceId === '' ? null : {traceId: traceId, otherTraceId: (params.get('other') || '').trim()};
    }

    /**
     * Where each span lands on a strip `width` wide covering [fromMs, toMs], clipped to it and
     * never thinner than 3, so a one-minute episode in a month stays visible. A null end runs
     * to the strip's end.
     * @param {Array<{from: number, to: number | null}>} spans
     * @param {number} fromMs
     * @param {number} toMs
     * @param {number} width
     * @returns {Array<{x: number, w: number}>}
     */
    function timelineMarks(spans, fromMs, toMs, width) {
        const scale = width / Math.max(1, toMs - fromMs);
        return spans.map(function (span) {
            const start = Math.max(fromMs, span.from);
            const end = Math.min(toMs, span.to == null ? toMs : span.to);
            return {
                x: Math.min(Math.round((start - fromMs) * scale), width - 3),
                w: Math.max(3, Math.round((end - start) * scale))
            };
        });
    }

    /**
     * A median lag, often under a second and seldom over a minute: dur() rounds to whole
     * seconds, which would print 1.2 s as 1 s.
     * @param {number | null | undefined} ms
     * @returns {string}
     */
    function lagText(ms) {
        if (ms == null) return '—';
        if (ms < 1000) return Math.round(ms) + ' ms';
        if (ms < 60000) return (ms / 1000).toFixed(1) + ' s';
        return dur(ms);
    }

    /**
     * A closed episode's span: its start whole, its end as a clock when it falls on the same day.
     * @param {number} startMs
     * @param {number} endMs
     * @returns {string}
     */
    function episodeSpan(startMs, endMs) {
        const start = dtHuman(startMs);
        const end = dtHuman(endMs);
        return start + ' → ' + (end.slice(0, 10) === start.slice(0, 10) ? end.slice(11) : end);
    }
```

In the `global.PSL = {` block, after `readHandoff,`:

```js
        CORRELATION_RANGES,
        readCorrelationPair,
        correlationPairs,
        spansOverlap,
        episodeHandoffHash,
        pairTraceHash,
        readTraceHandoff,
        timelineMarks,
        lagText,
        episodeSpan,
```

- [ ] **Step 4: Run them to verify they pass**

Run: `node --test tests/launcher.test.js && npx --prefix . tsc --noEmit -p tsconfig.json`
Expected: PASS, every test green, and `tsc` silent (run `npm ci` at the repository root first if `node_modules/typescript` is missing).

- [ ] **Step 5: Commit**

```bash
git add PerfSentinelHub/wwwroot/launcher.js tests/launcher.test.js && git commit -S -m "feat(launcher): fold episodes into pairs and hand their windows over"
```

---

### Task 8: The Correlations screen

**Files:**
- Modify: `PerfSentinelHub/wwwroot/index.html` (a tab after Incidents)
- Modify: `PerfSentinelHub/wwwroot/app.js` (state, `render`, `onRoute`, `rerun`, `handoffBanner`, `renderNewScreen`, a new `screen: correlations` section before `// ---- screen: ack one finding`)
- Modify: `PerfSentinelHub/wwwroot/launcher.css` (append)
- Modify: `tests/browser/global-setup.ts` (import keys, `seedCorrelations`, the embed capture)
- Test: `tests/browser/demo/stills.spec.ts`

**Interfaces:**
- Consumes: Task 7's `PSL` helpers, `GET /api/correlations` (Task 6), `POST /api/import/correlations` (Task 5), `POST /api/import/incidents`, existing app.js helpers `el`, `svg`, `ruledOverline`, `helpDot`, `getJson`, `filterSelect`, `checkbox`, `skeletonTable`, `critGlyph`, `warningGlyph`, `infoGlyph`, `sourceName`, `saveFolds`, `incidentPage`, `incidentKey`, `foldKey`, `render`, `currentScreen`.
- Produces (app.js, used by Task 9): `state.correlationFilter = {range, service, source_id, active, pair_key}`, the route `#/correlations?pair_key=<key>`, `state.incidentCorrelations = {}`, `pairCell(episode)`, `episodeSpanCell(episode)`, `incidentEnd(incident)`, `loadCorrelations()`. CSS classes `pair`, `pair-side`, `pair-then`, `timeline`, `timeline-strip`, `timeline-incident`, `timeline-episode`, `timeline-axis`, `chip[data-type]`, `chip[data-state="active"]`. Demo data: a pair `order-svc` then `orders-svc` at `checkout-prod` with three episodes, the newest running through the busy daemon's incidents.

- [ ] **Step 1: Seed the demo Hub with episodes**

In `tests/browser/global-setup.ts`, in `startAll`, before the `const hub = spawn(` line:

```ts
    // One key for the two daemons the correlations screens need pushes on.
    const importKey = randomBytes(30).toString("base64url");
```

change the two source lines to:

```ts
                ...source(0, "checkout-prod", "Checkout production", "production", "daemon", BUSY_PORT,
                    {...ack("checkout"), ImportApiKey: importKey}),
```

```ts
                ...source(3, "search-prod", "Search production", "production", "daemon", DEAD_PORT,
                    {ImportApiKey: importKey}),
```

and after `await waitFor(\`${BASE}/health/ready\`, 90);`:

```ts
    await seedCorrelations(importKey);
```

Add the function after `pushGroupedFindings`:

```ts
// The correlations screen and an incident's correlations read episodes, which only a daemon
// on perf-sentinel 0.30.0 or later pushes, and no fake daemon here does. So they are pushed
// through the Hub's own import, as such a daemon would push them, around the busy daemon's
// incidents: a pair active through them that also fired twice earlier in the day, and a
// second pair that went quiet hours ago. The dead daemon gets one incident pushed with an
// episode that ended long before it, the block that says no correlation was active. Billing
// staging pushes none, the block that says so.
async function seedCorrelations(importKey: string): Promise<void> {
    const minute = 60_000;
    const hour = 60 * minute;
    const incidents = await poll(async () => {
        const response = await fetch(`${BASE}/api/incidents?source_id=checkout-prod&limit=10`);
        const rows = response.ok ? await response.json() as { window_from_ms: number }[] : [];
        return rows.length > 0 ? rows : undefined;
    }, Date.now() + 30_000, 300, "the Hub never read the busy daemon's incidents");
    const now = Date.now();
    const hex = (n: number) => n.toString(16).padStart(32, "0");
    const fixture = (name: string) => JSON.parse(readFileSync(join(__dirname, "demo", "fixtures", name), "utf8"));
    const findings = fixture("daemon-findings.json") as
        { finding: { type: string, service: string, pattern?: { template?: string } } }[];
    // Each side is a finding the busy daemon really reports, template included.
    const side = (type: string, service: string) => {
        const found = findings.find((envelope) => envelope.finding.type === type && envelope.finding.service === service);
        if (!found) throw new Error(`daemon-findings.json holds no ${type} on ${service}`);
        return {finding_type: type, service, template: found.finding.pattern?.template ?? "", grouping_key: null};
    };
    const pool = {source: side("n_plus_one_sql", "order-svc"), target: side("pool_saturation", "orders-svc")};
    const slow = {source: side("slow_http", "reports-svc"), target: side("n_plus_one_http", "order-svc")};
    const quiet = {source: side("redundant_sql", "cache-svc"), target: side("slow_sql", "reports-svc")};
    const episode = (n: number, pair: object, pairKey: number, startedAt: number, lastSeen: number,
                     endedAt: number | null, peak: number) => ({
        id: hex(0x100 + n), pair_key: hex(pairKey), ...pair,
        started_at_ms: startedAt, last_seen_ms: lastSeen, ended_at_ms: endedAt,
        peak_confidence: peak, peak_co_occurrences: Math.round(peak * 45),
        last_confidence: Math.round((peak - 0.04) * 100) / 100, last_co_occurrences: Math.round(peak * 40),
        median_lag_ms: 1200, sample_trace_id: hex(0xb00 + n), source_sample_trace_id: hex(0xc00 + n),
        gap_ms: 10 * minute
    });
    const {version} = fixture("daemon-status-busy.json") as { version: string };
    const push = async (route: string, source: string, body: object) => {
        const response = await fetch(`${BASE}/api/import/${route}?source_id=${source}`, {
            method: "POST",
            headers: {"content-type": "application/json", "x-api-key": importKey},
            body: JSON.stringify({producer_version: version, ...body})
        });
        if (!response.ok) throw new Error(`the ${route} push for ${source} answered ${response.status}`);
    };
    const firstWindow = Math.min(...incidents.map((incident) => incident.window_from_ms));
    await push("correlations", "checkout-prod", {
        episodes: [
            episode(1, pool, 0xa1, now - 20 * hour, now - 1170 * minute, now - 1170 * minute, 0.88),
            episode(2, pool, 0xa1, now - 8 * hour, now - 450 * minute, now - 450 * minute, 0.81),
            episode(3, pool, 0xa1, firstWindow - 20 * minute, now - minute, null, 0.92),
            episode(4, slow, 0xa2, now - 3 * hour, now - 156 * minute, now - 156 * minute, 0.74)
        ]
    });
    // The dead daemon's incident, pushed: its own read fails, a push does not need it.
    const busy = fixture("daemon-incidents-busy.json") as Record<string, unknown>[];
    const delta = now - 5 * hour - (busy[0].at_ms as number);
    const slide = (node: unknown): unknown => {
        if (Array.isArray(node)) return node.map(slide);
        if (node === null || typeof node !== "object") return node;
        return Object.fromEntries(Object.entries(node).map(([key, value]) =>
            [key, key.endsWith("_ms") && typeof value === "number" && value > IS_TIMESTAMP ? value + delta : slide(value)]));
    };
    await push("incidents", "search-prod",
        {incidents: [{...(slide(busy[0]) as object), id: hex(0xd1), service: "search-svc"}]});
    await push("correlations", "search-prod",
        {episodes: [episode(5, quiet, 0xa3, now - 9 * hour, now - 510 * minute, now - 510 * minute, 0.7)]});
}
```

In `captureEmbedFixtures`, add `"/api/correlations",` after `incidents` in the first `readAll` list.

- [ ] **Step 2: Write the failing stills**

Append to `tests/browser/demo/stills.spec.ts`:

```ts
// The pair whose episodes run through the busy daemon's incidents, unfolded.
async function openPair(page: Page): Promise<void> {
    await page.goto("/#/correlations");
    await settled(page, "Findings that keep following each other");
    await page.locator("button.row-toggle", {hasText: "orders-svc"}).click();
    await expect(page.locator(".daemon-detail:not([hidden]) .timeline-strip")).toBeVisible();
}

test("correlations", async ({page}, info) => {
    await openPair(page);
    const panel = page.locator(".daemon-detail:not([hidden]) .daemon-panel");
    await expect(panel.locator("rect.timeline-incident").first()).toBeAttached();
    await expect(panel.locator("tbody tr")).toHaveCount(3);
    await expect(panel.locator("a.chip[data-kind]").first()).toBeVisible();
    await expect(panel.getByRole("link", {name: "Explain the last pair"}).first()).toBeVisible();
    await page.screenshot({
        path: nameFor("launcher-correlations", info.project.name),
        fullPage: true,
        animations: "disabled"
    });
});

test("correlations hand an episode's window to a new analysis", async ({page}) => {
    await openPair(page);
    await page.getByRole("link", {name: "Analyse this window"}).first().click();
    await expect(page).toHaveURL(/#\/new\?from=\d+&to=\d+&service=order-svc&pair=[0-9a-f]{32}$/);
    await expect(page.locator(".banner", {hasText: "from the correlations screen"})).toBeVisible();
});

test("correlations hand the last pair's traces to a new analysis", async ({page}) => {
    await openPair(page);
    await page.getByRole("link", {name: "Explain the last pair"}).first().click();
    await expect(page).toHaveURL(/#\/new\?trace=[0-9a-f]{32}&other=[0-9a-f]{32}$/);
    await page.locator("button.source-row", {hasText: "Tempo EU"}).click();
    const trace = page.locator('input[placeholder="4bf92f3577b34da6a3ce929d0e0e4736"]');
    const first = await trace.inputValue();
    await page.getByRole("button", {name: "Use the other trace"}).click();
    await expect(trace).not.toHaveValue(first);
});

test("correlations open on the pair a link names", async ({page}) => {
    await openPair(page);
    const pairKey = await page.evaluate(() => fetch("/api/correlations?service=orders-svc&limit=1")
        .then((response) => response.json()).then((rows) => rows[0].pair_key as string));
    await page.goto(`/#/correlations?pair_key=${pairKey}`);
    await settled(page, "Findings that keep following each other");
    await expect(page.locator("button.row-toggle")).toHaveCount(1);
    await expect(page.locator("button.row-toggle")).toHaveAttribute("aria-expanded", "true");
    // Back on every pair, over the 180 days the link widened to: three pairs on two daemons.
    await page.getByRole("link", {name: "Show every pair"}).click();
    await expect(page.locator("button.row-toggle")).toHaveCount(3);
});

test("correlations on a phone", async ({page}, info) => {
    await page.setViewportSize({width: 390, height: 844});
    await openPair(page);
    // The tables scroll inside their frame. The page itself must not.
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    await page.screenshot({
        path: nameFor("launcher-correlations-phone", info.project.name),
        fullPage: true,
        animations: "disabled"
    });
});
```

Add `Page` to the first import line: `import {expect, Page, test} from "@playwright/test";`.

- [ ] **Step 3: Run them to verify they fail**

Run: `cd tests/browser && DOTNET_ROOT="$HOME/.dotnet" npx playwright test --config playwright.demo.config.ts --project launcher-stills-light -g "correlations"`
Expected: FAIL, every new test times out in `settled()` because `#main` never shows "Findings that keep following each other" (`#/correlations` routes to the missing screen). The setup itself passes, the pushes answer 200.

- [ ] **Step 4: Add the tab**

In `index.html`, after the Incidents `<a class="shell-tab" …>` block:

```html
        <a class="shell-tab" data-screen="correlations" href="#/correlations">
            <svg aria-hidden="true" fill="none" stroke="currentColor" stroke-linecap="round"
                 stroke-linejoin="round" stroke-width="1.9" viewBox="0 0 24 24">
                <circle cx="6" cy="7" r="2.5"></circle>
                <circle cx="18" cy="17" r="2.5"></circle>
                <path d="M8.2 8.6l7.6 6.8"></path>
            </svg>
            Correlations
        </a>
```

- [ ] **Step 5: Wire the screen into app.js**

In `state`, after `incidentDetails: {},`:

```js
        // The correlations screen: the episodes of the range, null until read, the incidents
        // its strips draw, every service seen so the select outlives its own filter, and the
        // filters. An unfolded incident's episodes sit apart, by incident key: 'loading',
        // 'error', or {rows, pushes}.
        correlations: null,
        correlationsError: false,
        correlationIncidents: [],
        correlationServices: [],
        // pair_key mirrors the route, `#/correlations?pair_key=…`, and is never set from a select.
        correlationFilter: {range: '24h', service: '', source_id: '', active: false, pair_key: ''},
        incidentCorrelations: {},
        // A trace an episode handed to New analysis, with the pair's other one.
        traceHandoff: null,
```

In `render()`, after the incidents line:

```js
        else if (state.screen === 'correlations') main.replaceChildren(renderCorrelationsScreen());
```

In `onRoute()`, after `applyHandoff(PSL.readHandoff(location.hash, Date.now()));`:

```js
        applyTraceHandoff(PSL.readTraceHandoff(location.hash));
```

and after `else if (screen === 'incidents') loadIncidents();`:

```js
        else if (screen === 'correlations') {
            // A link to one pair, from Grafana or an incident, reads it over the widest range.
            state.correlationFilter.pair_key = PSL.readCorrelationPair(location.hash);
            if (state.correlationFilter.pair_key) state.correlationFilter.range = PSL.CORRELATION_RANGES[PSL.CORRELATION_RANGES.length - 1];
            loadCorrelations();
        }
```

In `rerun(run)`, after `state.handoff = null;`:

```js
        state.traceHandoff = null;
```

After `applyHandoff`, add:

```js
    /**
     * Puts a trace an episode handed over in the form. A route without one keeps the banner
     * while the form still holds one of the pair's two traces.
     */
    function applyTraceHandoff(handoff) {
        if (!handoff) {
            const kept = state.traceHandoff;
            if (kept && state.form.traceId !== kept.traceId && state.form.traceId !== kept.otherTraceId) {
                state.traceHandoff = null;
            }
            return;
        }
        state.traceHandoff = handoff;
        state.form.mode = 'trace';
        state.form.traceId = handoff.traceId;
        state.form.service = '';
        state.form.pickerOpen = false;
    }
```

In `handoffBanner`, replace the `text:` of the inner `el('div', …)` so an episode's window is named as one:

```js
        // An episode's window names no incident: it is said as the episode it came from.
        const origin = handoff.pairKey
            ? 'a correlation episode of ' + name + ' from the correlations screen, '
            : 'the ' + (kind || 'incident') + ' of ' + name + ' from the incidents screen, ';
```

declared after `const kind = …`, and the text expression becomes `'Window of ' + origin + span + (handoff.namespace ? … : '') + (daemon ? … : '')`, the two trailing parts unchanged.

After `handoffBanner`, add:

```js
    /** Where a trace in the form came from, and the pair's other trace one click away. */
    function traceHandoffBanner(source) {
        const handoff = state.traceHandoff;
        if (!handoff) return null;
        const daemon = Boolean(source && source.kind === 'daemon');
        const other = state.form.traceId === handoff.traceId ? handoff.otherTraceId : handoff.traceId;
        const swap = other ? el('button', {type: 'button', class: 'pill-button pill-sm', text: 'Use the other trace'})
            : null;
        if (swap) {
            swap.addEventListener('click', function () {
                state.form.traceId = other;
                render();
            });
        }
        return el('div', {class: 'banner', 'data-tone': daemon ? 'warn' : 'info'}, [
            daemon ? warningGlyph(16) : infoGlyph(16),
            el('div', {}, [
                el('p', {
                    text: 'A trace of the last co-occurrence of a correlated pair, from the correlations screen.'
                        + (other ? ' The pair\'s other trace is ' + other + '.' : '')
                        + (daemon ? ' A daemon takes no trace ID. Pick a trace backend on the left to run it.' : '')
                }),
                swap
            ])
        ]);
    }
```

In `renderNewScreen`, the right column becomes:

```js
        const right = el('div', {class: 'new-column'},
            [handoffBanner(source), traceHandoffBanner(source), parametersPanel(), costBand()]);
```

- [ ] **Step 6: Add the screen**

Before `// ------------------------------------------------- screen: ack one finding`:

```js
    // ---------------------------------------------------- screen: correlations

    const CORRELATION_LIMIT = 1000;
    const CORRELATION_COLUMNS = ['Pair', 'Episodes', 'Last active', 'Peak', 'Lag', 'Source'];
    const CORRELATION_COLUMNS_RIGHT = ['Episodes', 'Peak', 'Lag'];
    const EPISODE_COLUMNS = ['Episode', 'Lasted', 'Peak conf.', 'Peak co-occ.', 'Incidents', ''];
    const EPISODE_COLUMNS_RIGHT = ['Lasted', 'Peak conf.', 'Peak co-occ.'];
    const STRIP_WIDTH = 640;

    function correlationsPath() {
        const filter = state.correlationFilter;
        return '/api/correlations?limit=' + CORRELATION_LIMIT
            + '&from=' + (Date.now() - PSL.parseDur(filter.range))
            + (filter.service ? '&service=' + encodeURIComponent(filter.service) : '')
            + (filter.source_id ? '&source_id=' + encodeURIComponent(filter.source_id) : '')
            + (filter.pair_key ? '&pair_key=' + encodeURIComponent(filter.pair_key) : '')
            + (filter.active ? '&active=true' : '');
    }

    /**
     * The episodes of the range and the incidents the strips draw beside them, both from the
     * store: this screen never reads the fleet. The incidents are the store's newest page, so
     * a strip over an older range draws the ones that page still holds.
     */
    function loadCorrelations() {
        state.correlations = null;
        state.correlationsError = false;
        render();
        Promise.all([
            getJson(correlationsPath()),
            getJson('/api/incidents?limit=' + incidentPage() + '&offset=0').catch(function () {
                return [];
            })
        ]).then(function (results) {
            state.correlations = results[0];
            state.correlationIncidents = results[1];
            // A link to one pair lands on it unfolded, on every daemon that saw it.
            if (state.correlationFilter.pair_key) {
                results[0].forEach(function (episode) {
                    state.panelOpen['pair:' + episode.source_id + ':' + episode.pair_key] = true;
                });
            }
            results[0].forEach(function (episode) {
                [episode.source.service, episode.target.service].forEach(function (service) {
                    if (state.correlationServices.indexOf(service) < 0) state.correlationServices.push(service);
                });
            });
            state.correlationServices.sort();
        }).catch(function () {
            state.correlationsError = true;
            state.correlations = [];
        }).finally(function () {
            if (currentScreen() === 'correlations') render();
        });
    }

    function renderCorrelationsScreen() {
        const section = el('section', {}, [
            ruledOverline('// correlations'),
            el('h1', {class: 'page-title', text: 'Findings that keep following each other'}),
            el('p', {class: 'page-sub'}, [
                document.createTextNode('Each pair is a finding that, again and again, fires within seconds after '
                    + 'another one. Its periods of activity stay here after the daemon forgets them. '),
                helpDot('A daemon with [daemon.correlation] on and [daemon.hub_export] set pushes each period a '
                    + 'pair stays active as an episode, and closes it once the pair has not co-occurred for its '
                    + 'gap, ten minutes by default. The Hub keeps what it is pushed and re-derives nothing. An '
                    + 'episode whose daemon went silent past its gap and a quarter of an hour reads as ended '
                    + 'where it was last seen.', 'correlations')
            ]),
            correlationFilterLine()
        ]);
        if (state.correlations === null) {
            section.appendChild(el('div', {class: 'sources-wrap'}, [skeletonTable()]));
            return section;
        }
        if (state.correlationsError) {
            section.appendChild(el('div', {class: 'banner', 'data-tone': 'crit'}, [
                critGlyph(16),
                el('div', {text: 'The Hub did not return the correlations. This is the Hub itself, not any daemon.'})
            ]));
            return section;
        }
        const pairs = PSL.correlationPairs(state.correlations);
        if (pairs.length === 0) {
            section.appendChild(el('div', {class: 'empty-state'}, [
                el('p', {class: 'empty-title', text: 'No correlation in this range.'}),
                el('p', {
                    text: 'A pair shows here once a daemon with [daemon.correlation] and [daemon.hub_export] on '
                        + 'pushes an episode of it.'
                })
            ]));
            return section;
        }
        section.appendChild(el('div', {class: 'sources-wrap'}, [correlationsTable(pairs)]));
        if (state.correlations.length === CORRELATION_LIMIT) {
            section.appendChild(el('p', {
                class: 'sources-note',
                text: 'The Hub returned the ' + CORRELATION_LIMIT + ' most recent episodes of this range and stopped '
                    + 'there. Narrow the range or the filters to see the others.'
            }));
        }
        section.appendChild(el('p', {class: 'sources-note'}, [
            el('span', {text: 'Pushed by daemons with '}),
            el('span', {class: 'code-inline', text: '[daemon.hub_export]'}),
            el('span', {text: ' and '}),
            el('span', {class: 'code-inline', text: '[daemon.correlation]'}),
            el('span', {text: ' on. A daemon that pushes none has no rows here.'})
        ]));
        return section;
    }

    function correlationFilterLine() {
        const filter = state.correlationFilter;
        const set = function (key) {
            return function (value) {
                filter[key] = value;
                loadCorrelations();
            };
        };
        const range = el('select', {class: 'refresh-select', 'aria-label': 'range'},
            PSL.CORRELATION_RANGES.map(function (value) {
                const option = el('option', {value: value, text: 'last ' + PSL.humanDur(value)});
                if (value === filter.range) option.selected = true;
                return option;
            }));
        range.addEventListener('change', function () {
            set('range')(range.value);
        });
        const daemons = (state.sources || []).filter(function (source) {
            return source.kind === 'daemon';
        });
        // Shown only on a pair link: the way back to every pair is the plain route.
        const pairOnly = filter.pair_key
            ? el('a', {class: 'pill-button pill-sm', href: '#/correlations'}, [el('span', {text: 'Show every pair'})])
            : null;
        return el('div', {class: 'refresh'}, [
            pairOnly,
            el('span', {class: 'refresh-label', text: 'range'}),
            range,
            filterSelect('service', filter.service, state.correlationServices.map(function (service) {
                return [service, service];
            }), set('service')),
            filterSelect('daemon', filter.source_id, daemons.map(function (source) {
                return [source.id, source.name];
            }), set('source_id')),
            checkbox(filter.active, 'active now only', set('active'))
        ]);
    }

    function correlationsTable(pairs) {
        const head = el('tr', {}, CORRELATION_COLUMNS.map(function (name) {
            return el('th', {
                text: name, scope: 'col', 'data-align': CORRELATION_COLUMNS_RIGHT.indexOf(name) >= 0 ? 'right' : null
            });
        }));
        return el('table', {class: 'table'}, [el('thead', {}, [head]), el('tbody', {}, pairs.flatMap(pairRow))]);
    }

    /** Where an incident's span stops: its end once the alerting sent one, its window's end before. */
    function incidentEnd(incident) {
        return incident.ended_at_ms == null ? incident.window_to_ms : incident.ended_at_ms;
    }

    function pairRow(pair) {
        const fold = 'pair:' + pair.key;
        const id = 'pair-detail-' + pair.key.replace(/[^A-Za-z0-9_-]/g, '-');
        const open = state.panelOpen[fold] === true;
        const button = el('button', {
            type: 'button',
            class: 'row-toggle',
            'aria-expanded': open ? 'true' : 'false',
            'aria-controls': id,
            'aria-label': pair.last.source.service + ', then ' + pair.last.target.service
        }, [pairCell(pair.last)]);
        button.addEventListener('click', function () {
            const next = button.getAttribute('aria-expanded') !== 'true';
            button.setAttribute('aria-expanded', next ? 'true' : 'false');
            state.panelOpen[fold] = next;
            saveFolds();
            const cell = document.getElementById(id);
            if (cell) cell.parentNode.hidden = !next;
        });
        const row = el('tr', {}, [
            el('td', {}, [button]),
            el('td', {'data-align': 'right', class: 'table-mono', text: String(pair.episodes.length)}),
            episodeSpanCell(pair.last),
            el('td', {'data-align': 'right', class: 'table-mono', text: pair.peak.toFixed(2)}),
            el('td', {'data-align': 'right', class: 'table-mono', text: PSL.lagText(pair.last.median_lag_ms)}),
            el('td', {class: 'table-strong', text: sourceName(pair.source_id)})
        ]);
        const detail = el('tr', {class: 'daemon-detail'}, [
            el('td', {id: id, colspan: String(CORRELATION_COLUMNS.length)}, [pairPanel(pair)])
        ]);
        detail.hidden = !open;
        return [row, detail];
    }

    /** Both sides of a pair, the one that fires first on top, each template in its tooltip. */
    function pairCell(episode) {
        return el('span', {class: 'pair'}, [pairSide(episode.source, null), pairSide(episode.target, '↳ then')]);
    }

    function pairSide(side, lead) {
        return el('span', {class: 'pair-side', title: side.template || null}, [
            lead ? el('span', {class: 'pair-then', text: lead}) : null,
            el('span', {
                class: 'chip',
                'data-type': side.finding_type,
                text: PSL.FINDING_TYPE_LABEL[side.finding_type] || side.finding_type
            }),
            el('span', {class: 'table-strong', text: side.service})
        ]);
    }

    function episodeSpanCell(episode) {
        const cell = el('td', {
            class: 'table-mono',
            title: episode.ended_by === 'stale'
                ? 'The daemon stopped reporting this episode before closing it, so it reads as ended where it was '
                + 'last seen.'
                : null
        });
        if (episode.ended_at_ms == null) {
            cell.appendChild(document.createTextNode(PSL.dtHuman(episode.started_at_ms) + ' → '));
            cell.appendChild(el('span', {class: 'chip', 'data-state': 'active', text: 'active'}));
        } else {
            cell.appendChild(document.createTextNode(PSL.episodeSpan(episode.started_at_ms, episode.ended_at_ms)));
        }
        return cell;
    }

    function pairPanel(pair) {
        const toMs = Date.now();
        const fromMs = toMs - PSL.parseDur(state.correlationFilter.range);
        const incidents = state.correlationIncidents.filter(function (incident) {
            return incident.source_id === pair.source_id
                && PSL.spansOverlap(incident.window_from_ms, incidentEnd(incident), fromMs, toMs);
        });
        return el('div', {class: 'daemon-panel'}, [
            timelineStrip(pair.episodes, incidents, fromMs, toMs),
            el('div', {class: 'sources-wrap'}, [episodesTable(pair.episodes, incidents)])
        ]);
    }

    /** The range as a strip: each episode in green, the same daemon's incidents in red behind them. */
    function timelineStrip(episodes, incidents, fromMs, toMs) {
        const ns = 'http://www.w3.org/2000/svg';
        const strip = document.createElementNS(ns, 'svg');
        strip.setAttribute('viewBox', '0 0 ' + STRIP_WIDTH + ' 30');
        strip.setAttribute('preserveAspectRatio', 'none');
        strip.setAttribute('class', 'timeline-strip');
        strip.setAttribute('role', 'img');
        strip.setAttribute('aria-label', episodes.length + ' episodes and ' + incidents.length
            + ' incidents over the last ' + PSL.humanDur(state.correlationFilter.range));
        const draw = function (marks, className, y, height) {
            marks.forEach(function (mark) {
                const rect = document.createElementNS(ns, 'rect');
                rect.setAttribute('x', String(mark.x));
                rect.setAttribute('width', String(mark.w));
                rect.setAttribute('y', String(y));
                rect.setAttribute('height', String(height));
                rect.setAttribute('rx', '3');
                rect.setAttribute('class', className);
                strip.appendChild(rect);
            });
        };
        draw(PSL.timelineMarks(incidents.map(function (incident) {
            return {from: incident.window_from_ms, to: incidentEnd(incident)};
        }), fromMs, toMs, STRIP_WIDTH), 'timeline-incident', 2, 26);
        draw(PSL.timelineMarks(episodes.map(function (episode) {
            return {from: episode.started_at_ms, to: episode.ended_at_ms};
        }), fromMs, toMs, STRIP_WIDTH), 'timeline-episode', 9, 12);
        return el('div', {class: 'timeline'}, [
            strip,
            el('div', {class: 'timeline-axis'}, [
                el('span', {text: PSL.dtHuman(fromMs)}),
                el('span', {text: 'now'})
            ])
        ]);
    }

    function episodesTable(episodes, incidents) {
        // Explain asks for a trace ID, which only a trace backend can resolve.
        const traceable = (state.sources || []).some(function (source) {
            return source.kind !== 'daemon';
        });
        const now = Date.now();
        const head = el('tr', {}, EPISODE_COLUMNS.map(function (name) {
            return el('th', {
                text: name, scope: 'col', 'data-align': EPISODE_COLUMNS_RIGHT.indexOf(name) >= 0 ? 'right' : null
            });
        }));
        const rows = episodes.map(function (episode) {
            const overlapped = incidents.filter(function (incident) {
                return PSL.spansOverlap(episode.started_at_ms, episode.ended_at_ms,
                    incident.window_from_ms, incidentEnd(incident));
            });
            const end = episode.ended_at_ms == null ? now : episode.ended_at_ms;
            return el('tr', {}, [
                episodeSpanCell(episode),
                el('td', {'data-align': 'right', class: 'table-mono', text: PSL.dur(end - episode.started_at_ms)}),
                el('td', {'data-align': 'right', class: 'table-mono', text: episode.peak_confidence.toFixed(2)}),
                el('td', {'data-align': 'right', class: 'table-mono', text: String(episode.peak_co_occurrences)}),
                el('td', {}, overlapped.length === 0
                    ? [el('span', {class: 'table-muted', text: 'none'})]
                    : overlapped.map(incidentChip)),
                el('td', {}, [episodeActions(episode, traceable)])
            ]);
        });
        return el('table', {class: 'table'}, [el('thead', {}, [head]), el('tbody', {}, rows)]);
    }

    function episodeActions(episode, traceable) {
        const traces = traceable ? PSL.pairTraceHash(episode) : null;
        return el('div', {class: 'refresh'}, [
            el('a', {class: 'pill-button pill-sm', href: PSL.episodeHandoffHash(episode, Date.now())},
                [el('span', {text: 'Analyse this window'})]),
            traces ? el('a', {class: 'pill-button pill-sm', href: traces},
                [el('span', {text: 'Explain the last pair'})]) : null
        ]);
    }

    /** An incident an episode overlaps, as a link that opens it unfolded on the incidents screen. */
    function incidentChip(incident) {
        const link = el('a', {
            class: 'chip',
            'data-kind': incident.kind,
            href: '#/incidents',
            text: (PSL.INCIDENT_KIND_LABEL[incident.kind] || incident.kind) + ' · ' + incident.service + ' · '
                + PSL.dtHuman(incident.at_ms)
        });
        link.addEventListener('click', function () {
            state.incidentFilter = {service: '', namespace: '', kind: '', environment: '', source_id: incident.source_id};
            state.panelOpen[foldKey(incident)] = true;
            saveFolds();
        });
        return link;
    }
```

- [ ] **Step 7: Style it with the existing tokens**

Append to `launcher.css`:

```css
/* A correlated pair: the side that fires first, then the one that follows it. */
.pair {
    display: inline-flex;
    flex-direction: column;
    gap: 3px;
    text-align: left;
}

.pair-side {
    display: inline-flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 7px;
}

.pair-then {
    color: var(--text-3);
    font-size: 12px;
}

/* A finding type takes the tone the incident kinds already use for its family. */
.chip[data-type^="n_plus_one"] {
    background: var(--info-bg);
    color: var(--info-fg);
}

.chip[data-type="pool_saturation"] {
    background: var(--crit-bg);
    color: var(--crit-fg);
}

.chip[data-type^="slow_"] {
    background: var(--warn-bg);
    color: var(--warn-fg);
}

.chip[data-state="active"] {
    background: var(--ok-bg);
    color: var(--ok-fg);
}

a.chip {
    text-decoration: none;
}

.timeline {
    margin: 4px 0 14px;
}

.timeline-strip {
    display: block;
    width: 100%;
    height: 30px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
}

.timeline-incident {
    fill: var(--crit-bg);
}

.timeline-episode {
    fill: var(--brand);
}

.timeline-axis {
    display: flex;
    justify-content: space-between;
    margin-top: 4px;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-3);
}
```

- [ ] **Step 8: Run the stills to verify they pass**

Run: `cd tests/browser && DOTNET_ROOT="$HOME/.dotnet" npx playwright test --config playwright.demo.config.ts --project launcher-stills-light --project launcher-stills-dark -g "correlations"`, then `node --test tests/launcher.test.js` from the repository root.
Expected: PASS, 10 tests (5 per theme), and the unit tests still green. The run writes `docs/img/hub/launcher-correlations*.png`: leave them unstaged, Task 10 regenerates every screenshot together.

If the phone test fails on the page width, find the element wider than 390 px (`[...document.querySelectorAll('*')].filter(n => n.getBoundingClientRect().right > 390)` in the Playwright trace) and give it `min-width: 0` or move it inside `.sources-wrap`. Do not widen the viewport.

- [ ] **Step 9: Commit**

```bash
git add PerfSentinelHub/wwwroot/index.html PerfSentinelHub/wwwroot/app.js PerfSentinelHub/wwwroot/launcher.css tests/browser/global-setup.ts tests/browser/demo/stills.spec.ts && git commit -S -m "feat(launcher): add the Correlations screen"
```

---

### Task 9: Correlations active during an incident

**Files:**
- Modify: `PerfSentinelHub/wwwroot/app.js` (`incidentPanel`, the correlations section)
- Modify: `PerfSentinelHub/wwwroot/launcher.css` (append)
- Test: `tests/browser/demo/stills.spec.ts`

**Interfaces:**
- Consumes: `pairCell`, `episodeSpanCell`, `incidentEnd`, `state.incidentCorrelations`, the route `#/correlations?pair_key=<key>` (Task 8), `incidentKey`, `incidentPanel`, `getJson`.
- Produces: the block `.incident-correlations` in an incident's detail, with its rows, "All its episodes" links and the two empty states.

- [ ] **Step 1: Write the failing stills**

Append to `tests/browser/demo/stills.spec.ts`:

```ts
test("correlations during an incident", async ({page}, info) => {
    await page.goto("/#/incidents");
    await settled(page, "What was already burning");
    await page.locator("button.row-toggle", {hasText: "checkout-svc"}).click();
    const block = page.locator(".daemon-detail:not([hidden]) .incident-correlations");
    await expect(block.locator("tbody tr")).toHaveCount(1, {timeout: 15_000});
    await expect(block.getByRole("link", {name: "All its episodes"})).toBeVisible();
    await page.screenshot({
        path: nameFor("launcher-incident-correlations", info.project.name),
        fullPage: true,
        animations: "disabled"
    });
});

test("correlations an incident cannot show", async ({page}) => {
    await page.goto("/#/incidents");
    await settled(page, "What was already burning");
    await page.locator("button.row-toggle", {hasText: "search-svc"}).click();
    await expect(page.locator(".incident-correlations", {hasText: "No correlation was active in this window."}))
        .toBeVisible({timeout: 15_000});
    await page.locator("button.row-toggle", {hasText: "cache-svc"}).click();
    await expect(page.locator(".incident-correlations",
        {hasText: "Billing staging pushes no correlations, so none can show here."})).toBeVisible({timeout: 15_000});
});

test("correlations open every episode of an incident's pair", async ({page}) => {
    await page.goto("/#/incidents");
    await settled(page, "What was already burning");
    await page.locator("button.row-toggle", {hasText: "checkout-svc"}).click();
    await page.locator(".daemon-detail:not([hidden]) .incident-correlations")
        .getByRole("link", {name: "All its episodes"}).click();
    await expect(page).toHaveURL(/#\/correlations\?pair_key=[0-9a-f]{32}$/);
    await settled(page, "Findings that keep following each other");
    await expect(page.locator("button.row-toggle", {hasText: "orders-svc"})).toHaveAttribute("aria-expanded", "true");
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd tests/browser && DOTNET_ROOT="$HOME/.dotnet" npx playwright test --config playwright.demo.config.ts --project launcher-stills-light -g "an incident|incident's pair|during an incident"`
Expected: FAIL, `.incident-correlations` never appears in an unfolded incident.

- [ ] **Step 3: Add the block**

In `incidentPanel`, the returned panel ends with the block. The last child of the `el('div', {class: 'daemon-panel'}, [ … ])` list becomes:

```js
            findings.length === 0 ? null : el('div', {class: 'sources-wrap'}, [incidentFindingsTable(findings, detail)]),
            incidentCorrelationsBlock(incident)
```

Append to the correlations section of `app.js`:

```js
    /**
     * The episodes of an incident's daemon active during it, read once per unfolded incident.
     * An empty read asks once more whether the daemon pushes any at all, so a daemon without
     * the push never reads as a quiet one.
     */
    function loadIncidentCorrelations(incident) {
        const key = incidentKey(incident);
        state.incidentCorrelations[key] = 'loading';
        const scope = '/api/correlations?source_id=' + encodeURIComponent(incident.source_id);
        getJson(scope + '&from=' + incident.window_from_ms + '&to=' + incidentEnd(incident))
            .then(function (rows) {
                if (rows.length > 0) return {rows: rows, pushes: true};
                return getJson(scope + '&limit=1').then(function (any) {
                    return {rows: rows, pushes: any.length > 0};
                });
            })
            .then(function (result) {
                state.incidentCorrelations[key] = result;
            })
            .catch(function () {
                state.incidentCorrelations[key] = 'error';
            })
            .finally(function () {
                const cell = document.getElementById('incident-detail-' + key);
                if (cell) cell.replaceChildren(incidentPanel(incident));
            });
    }

    function incidentCorrelationsBlock(incident) {
        const held = state.incidentCorrelations[incidentKey(incident)];
        if (held === undefined) {
            queueMicrotask(function () {
                loadIncidentCorrelations(incident);
            });
        }
        const block = el('div', {class: 'incident-correlations'}, [
            el('p', {class: 'overline daemon-audience', text: '// correlations'}),
            el('h3', {class: 'daemon-lead', text: 'Correlations active during this incident'}),
            el('p', {
                class: 'daemon-source-note',
                text: 'Episodes whose activity overlaps the incident\'s window, from the same daemon.'
            })
        ]);
        if (held === undefined || held === 'loading') {
            block.appendChild(el('p', {class: 'daemon-loading', role: 'status', text: 'Reading the correlations.'}));
        } else if (held === 'error') {
            block.appendChild(el('p', {class: 'table-muted', text: 'The Hub did not return the correlations of this window.'}));
        } else if (held.rows.length > 0) {
            block.appendChild(el('div', {class: 'sources-wrap'}, [incidentEpisodesTable(held.rows)]));
        } else {
            block.appendChild(el('p', {
                class: 'empty-state',
                text: held.pushes
                    ? 'No correlation was active in this window.'
                    : (incident.source_name || incident.source_id) + ' pushes no correlations, so none can show here.'
            }));
        }
        return block;
    }

    function incidentEpisodesTable(episodes) {
        const head = el('tr', {}, ['Pair', 'Episode', 'Peak', ''].map(function (name) {
            return el('th', {text: name, scope: 'col', 'data-align': name === 'Peak' ? 'right' : null});
        }));
        const rows = episodes.map(function (episode) {
            return el('tr', {}, [
                el('td', {}, [pairCell(episode)]),
                episodeSpanCell(episode),
                el('td', {'data-align': 'right', class: 'table-mono', text: episode.peak_confidence.toFixed(2)}),
                el('td', {}, [allEpisodesLink(episode)])
            ]);
        });
        return el('table', {class: 'table'}, [el('thead', {}, [head]), el('tbody', {}, rows)]);
    }

    /** Every episode of an episode's pair, through the route the Grafana table links to. */
    function allEpisodesLink(episode) {
        return el('a', {
            class: 'pill-button pill-sm',
            href: '#/correlations?pair_key=' + encodeURIComponent(episode.pair_key)
        }, [el('span', {text: 'All its episodes'})]);
    }
```

Append to `launcher.css`:

```css
/* An incident's correlations, under its frozen findings and set apart from them. */
.incident-correlations {
    display: grid;
    gap: 10px;
    margin-top: 18px;
    padding-top: 16px;
    border-top: 1px solid var(--border);
}
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cd tests/browser && DOTNET_ROOT="$HOME/.dotnet" npx playwright test --config playwright.demo.config.ts --project launcher-stills-light --project launcher-stills-dark`
Expected: PASS, every still in both themes, the incidents and handoff stills included (the incident panel now ends with the block, and its "Analyse this window" link is still the first one).

- [ ] **Step 5: Commit**

```bash
git add PerfSentinelHub/wwwroot/app.js PerfSentinelHub/wwwroot/launcher.css tests/browser/demo/stills.spec.ts && git commit -S -m "feat(launcher): list the correlations active during an incident"
```

---

### Task 10: Documentation, changelog and screenshots

**Files:**
- Modify: `docs/API.md` and `docs/FR/API-FR.md` (Import API, Read API table, a Correlations subsection before "What the Hub adds to a finding")
- Modify: `docs/OPERATIONS.md` and `docs/FR/OPERATIONS-FR.md` (metrics table)
- Modify: `docs/ARCHITECTURE.md` and `docs/FR/ARCHITECTURE-FR.md` (push and poll, retention clocks, database table)
- Modify: `docs/AUTHENTICATION.md` and `docs/FR/AUTHENTICATION-FR.md`, `docs/ACCESS-CONTROL.md` and `docs/FR/ACCESS-CONTROL-FR.md`, `docs/LIMITATIONS.md` and `docs/FR/LIMITATIONS-FR.md` (the route inventories)
- Modify: `docs/LAUNCHER.md` and `docs/FR/LAUNCHER-FR.md` (intro, a Correlations section)
- Modify: `CHANGELOG.md` (`## [Unreleased]`)
- Regenerate: `docs/img/hub/*`, `tests/browser/demo/fixtures/hub-embed.json`

**Interfaces:**
- Consumes: everything above.
- Produces: the documented contract the engine plan and the Grafana panel link to.

In every table below, match the existing column widths (pad the cells, widen the separator row when a new cell is longer).

- [ ] **Step 1: API.md.** After the `POST /api/import/incidents` paragraph:

```markdown
`POST /api/import/correlations?source_id=<id>` accepts the correlation episodes a daemon on
perf-sentinel 0.30.0 or later pushes when `[daemon.correlation]` is on beside
`[daemon.hub_export]`: `{"producer_version":"…","episodes":[…]}`, each episode one period during
which a pair of findings kept following each other. It takes the same key, the same four-slot gate
and the same five-second write wait, 1 to 100 episodes and at most 1 MiB per request. Each episode
is checked on its own: a 32-character hex `id` and `pair_key`, a known finding type on both sides,
`started_at_ms` no later than `last_seen_ms`, an `ended_at_ms` absent or no earlier than the start,
confidences between 0 and 1. An invalid one is counted in `rejected` while the others land, and a
batch with none valid is a `400`. Copies of one episode merge in any order: the earliest start, the
latest `last_seen_ms` and the highest peaks win, the rolling values come from the newer copy, and
an end is kept once seen. Refusals count on `perf_sentinel_hub_correlation_import_rejected_total`.
```

Read API table row, after `POST /api/incidents/refresh`:

```markdown
| `GET /api/correlations`              | The correlation episodes the daemons pushed, newest first, filtered by `service`, `source_id`, `environment`, `pair_key`, `from`, `to` and `active`. See below |
```

New subsection before `### What the Hub adds to a finding`:

```markdown
### Correlations

A perf-sentinel daemon from 0.30.0 with `[daemon.correlation]` on and `[daemon.hub_export]` set
pushes each period during which a pair of findings keeps following each other as an episode, and
closes it once the pair has not co-occurred for its gap, `min(window, 30 min)` and 10 minutes by
default. The daemon cuts the episodes and the Hub stores them as pushed, keyed on the episode `id`
and the `source_id`. Nothing is polled: a daemon without `[daemon.hub_export]` leaves no
correlation history in the Hub.

`GET /api/correlations` answers a JSON array of episodes, newest first by `started_at_ms`, in the
shape the daemon pushes them: `id`, `pair_key`, `source` and `target` (each with `finding_type`,
`service`, `template`, `grouping_key` and `grouping_value`, the last two null for a finding with
no grouping), `started_at_ms`, `last_seen_ms`, `ended_at_ms`, `gap_ms`, the peaks
`peak_confidence` and `peak_co_occurrences`, the latest `last_confidence`, `last_co_occurrences`
and `median_lag_ms`, and the two sample traces `source_sample_trace_id` and `sample_trace_id`.
The Hub adds `source_id`, `source_name`, `ended_by`, and `first_seen_ms` and `last_seen_hub_ms`
on its own clock. `ended_at_ms` is null while the episode runs. An open episode whose
`last_seen_ms` is older than its `gap_ms` plus 15 minutes is served as ended at its
`last_seen_ms` with `ended_by` set to `stale`, most likely a daemon stopped before it could close
it, and `ended_by` is null on every other episode. The stored row keeps no end, so a late close
still lands. The same pair seen under two groupings has two `pair_key` values.

The listing filters on `service`, matched on either side, `pair_key`, `source_id`,
`environment`, `from` and `to`, which keep the episodes whose activity overlaps the span, bounds
included and a running episode included, and `active=true`, which keeps the running ones. A
filter given empty or as whitespace only is no filter, which is what a Grafana variable set to
All sends. It pages with `offset` and `limit`, 200 by default and 1000 at most. `source_id` and
`environment` are closed sets as on `/api/incidents`, and `from` and `to` are epoch
milliseconds, so a value the route cannot read is a `400`. The route asks for `findings.read`: with access control on,
an episode is listed only when the caller reads both its services at its source.
```

French mirror, same places in `docs/FR/API-FR.md`:

```markdown
`POST /api/import/correlations?source_id=<id>` accepte les épisodes de corrélation qu'un daemon en
perf-sentinel 0.30.0 ou suivant pousse quand `[daemon.correlation]` est actif à côté de
`[daemon.hub_export]` : `{"producer_version":"…","episodes":[…]}`, chaque épisode étant une période
pendant laquelle une paire de findings n'a pas cessé de se suivre. Il prend la même clé, la même
porte à quatre places et la même attente d'écriture de cinq secondes, de 1 à 100 épisodes et 1 Mio
au plus par requête. Chaque épisode est vérifié seul : un `id` et une `pair_key` de 32 caractères
hexadécimaux, un type de finding connu des deux côtés, un `started_at_ms` au plus égal à
`last_seen_ms`, un `ended_at_ms` absent ou au moins égal au début, des confiances entre 0 et 1. Un
épisode invalide est compté dans `rejected` pendant que les autres sont enregistrés, et un lot sans
aucun épisode valide est un `400`. Les copies d'un même épisode fusionnent dans n'importe quel
ordre : le début le plus ancien, le `last_seen_ms` le plus récent et les pics les plus hauts
l'emportent, les valeurs glissantes viennent de la copie la plus récente, et une fin est gardée une
fois vue. Les refus se comptent sur `perf_sentinel_hub_correlation_import_rejected_total`.
```

```markdown
| `GET /api/correlations`              | Les épisodes de corrélation poussés par les daemons, du plus récent au plus ancien, filtrés par `service`, `source_id`, `environment`, `pair_key`, `from`, `to` et `active`. Voir plus bas |
```

```markdown
### Corrélations

Un daemon perf-sentinel à partir de 0.30.0, avec `[daemon.correlation]` actif et
`[daemon.hub_export]` configuré, pousse comme un épisode chaque période pendant laquelle une paire
de findings se suit, et le ferme une fois que la paire n'a plus coïncidé pendant son écart,
`min(fenêtre, 30 min)` et 10 minutes par défaut. Le daemon découpe les épisodes et le Hub les
garde tels qu'ils sont poussés, sous la clé de l'`id` de l'épisode et du `source_id`. Rien n'est lu
par poll : un daemon sans `[daemon.hub_export]` ne laisse aucun historique de corrélation dans le
Hub.

`GET /api/correlations` répond un tableau JSON d'épisodes, du plus récent au plus ancien selon
`started_at_ms`, sous la forme où le daemon les pousse : `id`, `pair_key`, `source` et `target`
(chacun avec `finding_type`, `service`, `template`, `grouping_key` et `grouping_value`, ces deux
derniers à null pour un finding sans regroupement), `started_at_ms`, `last_seen_ms`,
`ended_at_ms`, `gap_ms`, les pics `peak_confidence` et `peak_co_occurrences`, les dernières
valeurs `last_confidence`, `last_co_occurrences` et `median_lag_ms`, et les deux traces d'exemple
`source_sample_trace_id` et `sample_trace_id`. Le Hub ajoute `source_id`, `source_name`,
`ended_by`, et `first_seen_ms` et `last_seen_hub_ms` sur sa propre horloge. `ended_at_ms` vaut
null tant que l'épisode dure. Un épisode ouvert dont le `last_seen_ms` est plus vieux que son
`gap_ms` plus 15 minutes est servi comme terminé à son `last_seen_ms`, avec `ended_by` à `stale`,
le plus souvent un daemon arrêté avant d'avoir pu le fermer, et `ended_by` vaut null sur tous les
autres épisodes. La ligne stockée ne garde aucune fin, donc une fermeture tardive arrive quand
même. Une même paire vue sous deux regroupements a deux `pair_key`.

La liste se filtre sur `service`, cherché des deux côtés, `pair_key`, `source_id`,
`environment`, `from` et `to`, qui gardent les épisodes dont l'activité recouvre l'intervalle,
bornes comprises et épisode en cours compris, et `active=true`, qui garde ceux en cours. Un filtre
vide ou fait d'espaces n'est pas un filtre, ce qu'envoie une variable Grafana réglée sur All. Elle
se pagine avec `offset` et `limit`, 200 par défaut et 1000 au plus. `source_id` et `environment`
sont des ensembles fermés comme sur `/api/incidents`, et `from` et `to` sont des millisecondes
depuis l'epoch, donc une valeur que la route ne sait pas lire est un `400`. La route demande
`findings.read` : avec le contrôle d'accès actif, un épisode n'est listé que si l'appelant lit ses
deux services à sa source.
```

- [ ] **Step 2: OPERATIONS.md.** Row after `perf_sentinel_hub_incident_import_rejected_total{reason}`:

```markdown
| `perf_sentinel_hub_correlation_import_rejected_total{reason}`          | counter | Correlation imports refused, by the findings reasons  |
```

French mirror:

```markdown
| `perf_sentinel_hub_correlation_import_rejected_total{reason}`          | counter | Imports de corrélations refusés, par les motifs des findings      |
```

- [ ] **Step 3: ARCHITECTURE.md.** After the paragraph starting "Incidents follow the same split":

```markdown
Correlation episodes take the push alone from perf-sentinel 0.30.0: the daemon cuts each episode and
pushes it when it opens, when a peak rises, when its last co-occurrence moves on by a minute and
when it closes. Nothing polls them, so a daemon that does not push leaves the Hub without them.
```

In "Three retention clocks", after the sentence ending "never on the alerting's `at_ms`.":

```markdown
Correlation episodes ride it as well, purged on their own `last_seen_ms`, the daemon's stamp of
the pair's last co-occurrence: an episode is history once it closes, and the Hub's clock only says
when a copy arrived.
```

Row in "What the database holds", after `incident_attributes`:

```markdown
| `correlation_episodes`         | episode and source                                             | retention, on `last_seen_ms`               |
```

French mirror, same places:

```markdown
Les épisodes de corrélation ne passent que par le push depuis perf-sentinel 0.30.0 : le daemon
découpe chaque épisode et le pousse quand il s'ouvre, quand un pic monte, quand sa dernière
co-occurrence avance d'une minute et quand il se ferme. Rien ne les lit par poll, donc un daemon
qui ne pousse pas laisse le Hub sans eux.
```

```markdown
Les épisodes de corrélation la suivent aussi, purgés sur leur propre `last_seen_ms`, l'horodatage
que le daemon donne à la dernière co-occurrence de la paire : un épisode devient de l'histoire une
fois fermé, et l'horloge du Hub ne dit que le moment où une copie est arrivée.
```

```markdown
| `correlation_episodes`         | épisode et source                                                    | rétention, sur `last_seen_ms`              |
```

- [ ] **Step 4: The route inventories.**

`docs/AUTHENTICATION.md`: the first session row becomes `` `/api/status`, `/api/sources`, `/api/incidents`, `/api/correlations`, `/api/analyses` `` and, after the `POST /api/import/incidents` row:

```markdown
| `POST /api/import/correlations`                                  | the same `X-API-Key` as the findings import                             |
```

French mirror: the same session row change, and

```markdown
| `POST /api/import/correlations`                                  | la même `X-API-Key` que l'import des findings                              |
```

`docs/ACCESS-CONTROL.md`, after the incidents refresh row:

```markdown
| `GET /api/correlations`                                                | `findings.read`, filtered to the episodes whose two services the caller reads at that source | `403` when held nowhere |
```

and the last row becomes `` `POST /api/import/findings`, `POST /api/import/incidents`, `POST /api/import/correlations`, `/health/*` | unchanged ``. French mirror:

```markdown
| `GET /api/correlations`                                                | `findings.read`, filtré sur les épisodes dont l'appelant lit les deux services à cette source | `403` s'il n'est détenu nulle part |
```

with the same change to the last row (`inchangé`).

`docs/LIMITATIONS.md`: "Two endpoints ask for a credential, `POST /api/import/findings` and `POST /api/import/incidents`," becomes "Three endpoints ask for a credential, `POST /api/import/findings`, `POST /api/import/incidents` and `POST /api/import/correlations`,", and `` `/api/findings`, `` in the list that follows becomes `` `/api/findings`, `/api/correlations`, ``. French mirror: "Deux endpoints réclament une preuve, `POST /api/import/findings` et `POST /api/import/incidents`," becomes "Trois endpoints réclament une preuve, `POST /api/import/findings`, `POST /api/import/incidents` et `POST /api/import/correlations`," with the same list change.

- [ ] **Step 5: LAUNCHER.md.** The intro "Five screens: … read the incidents the daemons recorded." becomes "Six screens: start an analysis, follow one run, list recent runs, read fleet health, read the incidents the daemons recorded, follow the correlations they pushed." and "an administrator gets a sixth tab" becomes "an administrator gets a seventh tab". New section before `## The ack page`:

```markdown
## The correlations screen

Each row is a pair of findings that kept following each other, one firing within seconds after the
other, as the daemons with `[daemon.correlation]` and `[daemon.hub_export]` on pushed it: the side
that fires first, the one that follows it, how many episodes the pair had in the range, the last
one, marked active while it runs, its peak confidence, the median lag between the two and the
daemon. The rows are built in the browser from the episodes of the range, a thousand at most, and a
line under the table says when the read stopped there. A range, the last 24 hours by default, a
service matched on either side, a daemon and an "active now only" switch narrow the list.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/Institut-du-Numerique-Responsable/PerfSentinelHub/main/docs/img/hub/launcher-correlations-dark.png">
  <img alt="The correlations screen with a pair unfolded on its strip and its episodes" src="https://raw.githubusercontent.com/Institut-du-Numerique-Responsable/PerfSentinelHub/main/docs/img/hub/launcher-correlations.png">
</picture>

A row unfolds into a strip of the range, each episode in green over the same daemon's incidents in
red, then the list of its episodes: span, duration, peaks and the incidents each one overlaps,
which open the incidents screen on that incident. Analyse this window opens New analysis on the
episode's window, a minute wider on each side, and on the service of the side that fires first, as
the incidents screen does. Explain the last pair, shown when the Hub has a trace backend, opens New
analysis on the trace of one finding of the pair's last co-occurrence, with the other trace one
click away in the banner.

An unfolded incident lists the correlations active during it: the episodes of its daemon that
overlap the incident's window, up to its end once it has one, each with a link to every episode of
its pair. That link, `#/correlations?pair_key=<key>`, is the one the Grafana correlations table
uses too: the screen opens on that pair over 180 days, unfolded, with a Show every pair button
to leave it. A daemon that pushed no correlation at all says so, so an empty block never reads as
a quiet one. On a phone the tables scroll sideways inside their frame, as the incident tables do, and
the page does not.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/Institut-du-Numerique-Responsable/PerfSentinelHub/main/docs/img/hub/launcher-incident-correlations-dark.png">
  <img alt="An unfolded incident listing the correlations active during it" src="https://raw.githubusercontent.com/Institut-du-Numerique-Responsable/PerfSentinelHub/main/docs/img/hub/launcher-incident-correlations.png">
</picture>
```

French mirror: the intro becomes "Six écrans : démarrer une analyse, suivre un run, lister les runs récents, lire la santé de la flotte, lire les incidents enregistrés par les daemons, suivre les corrélations qu'ils ont poussées." and "un sixième onglet" becomes "un septième onglet". New section before `## La page d'acquittement`:

```markdown
## L'écran des corrélations

Chaque ligne est une paire de findings qui n'ont pas cessé de se suivre, l'un se déclenchant
quelques secondes après l'autre, telle que l'ont poussée les daemons qui ont `[daemon.correlation]`
et `[daemon.hub_export]` actifs : le côté qui se déclenche en premier, celui qui le suit, le nombre
d'épisodes de la paire sur la période, le dernier, marqué actif tant qu'il dure, son pic de
confiance, le délai médian entre les deux et le daemon. Les lignes sont construites dans le
navigateur à partir des épisodes de la période, mille au plus, et une ligne sous le tableau dit
quand la lecture s'est arrêtée là. Une période, les dernières 24 heures par défaut, un service
cherché des deux côtés, un daemon et un interrupteur "active now only" resserrent la liste.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/Institut-du-Numerique-Responsable/PerfSentinelHub/main/docs/img/hub/launcher-correlations-dark.png">
  <img alt="L'écran des corrélations avec une paire dépliée sur sa frise et ses épisodes" src="https://raw.githubusercontent.com/Institut-du-Numerique-Responsable/PerfSentinelHub/main/docs/img/hub/launcher-correlations.png">
</picture>

Une ligne se déplie en une frise de la période, chaque épisode en vert sur les incidents du même
daemon en rouge, puis la liste de ses épisodes : durée, pics et incidents que chacun recouvre, qui
ouvrent l'écran des incidents sur cet incident. Analyse this window ouvre New analysis sur la
fenêtre de l'épisode, élargie d'une minute de chaque côté, et sur le service du côté qui se
déclenche en premier, comme le fait l'écran des incidents. Explain the last pair, affiché quand le
Hub a un backend de traces, ouvre New analysis sur la trace d'un finding de la dernière
co-occurrence de la paire, l'autre trace restant à un clic dans le bandeau.

Un incident déplié liste les corrélations actives pendant sa durée : les épisodes de son daemon qui
recouvrent la fenêtre de l'incident, jusqu'à sa fin dès qu'il en a une, chacun avec un lien vers
tous les épisodes de sa paire. Ce lien, `#/correlations?pair_key=<clé>`, est aussi celui du tableau
des corrélations de Grafana : l'écran s'ouvre sur cette paire sur 180 jours, dépliée, avec un
bouton Show every pair pour en sortir. Un daemon qui n'a poussé aucune corrélation le dit, pour qu'un bloc
vide ne se lise jamais comme un bloc calme. Sur un téléphone, les tableaux défilent latéralement
dans leur cadre, comme ceux des incidents, et la page ne défile pas.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/Institut-du-Numerique-Responsable/PerfSentinelHub/main/docs/img/hub/launcher-incident-correlations-dark.png">
  <img alt="Un incident déplié qui liste les corrélations actives pendant sa durée" src="https://raw.githubusercontent.com/Institut-du-Numerique-Responsable/PerfSentinelHub/main/docs/img/hub/launcher-incident-correlations.png">
</picture>
```

- [ ] **Step 6: CHANGELOG.md.** Under a new `## [Unreleased]` heading above `## [0.5.0] - 2026-10-15`:

```markdown
## [Unreleased]

### Added

- A Correlations screen and `GET /api/correlations` keep the periods during which a pair of
  findings kept following each other, as episodes a daemon on perf-sentinel 0.30.0 or later pushes
  to the new `POST /api/import/correlations?source_id=<id>` with `[daemon.correlation]` and
  `[daemon.hub_export]` on. The daemon cuts each episode, the Hub merges its copies in any order
  and purges it with the findings retention. A pair unfolds into a strip of its episodes beside the
  daemon's incidents, an episode hands its window to New analysis, and an unfolded incident lists
  the correlations active during it. The import takes the same key and gate as the two others, at
  most 100 episodes and 1 MiB per request, and refusals count on
  `perf_sentinel_hub_correlation_import_rejected_total{reason}`.
```

- [ ] **Step 7: Regenerate the screenshots and the embed fixture**

Run: `cd tests/browser && DOTNET_ROOT="$HOME/.dotnet" npm run demo`
Expected: exit 0, every project green, `docs/img/hub/launcher-correlations{,-dark,-phone,-phone-dark}.png` and `launcher-incident-correlations{,-dark}.png` written, the other stills and both GIFs refreshed, `tests/browser/demo/fixtures/hub-embed.json` gaining a `/api/correlations` key. Open the two new light stills and check them against the mockup (`.superpowers/brainstorm/156-1792096988/content/correlations-launcher-v4.html` in the engine repo).

- [ ] **Step 8: Verify and commit**

Run: `make verify-fast && make security` (pinned SDK on PATH)
Expected: exit 0 for both, format, tests, coverage ratchet, analysis-config, badges, security exceptions, audit and supply chain green.

```bash
git add docs/API.md docs/FR/API-FR.md docs/OPERATIONS.md docs/FR/OPERATIONS-FR.md docs/ARCHITECTURE.md docs/FR/ARCHITECTURE-FR.md docs/AUTHENTICATION.md docs/FR/AUTHENTICATION-FR.md docs/ACCESS-CONTROL.md docs/FR/ACCESS-CONTROL-FR.md docs/LIMITATIONS.md docs/FR/LIMITATIONS-FR.md docs/LAUNCHER.md docs/FR/LAUNCHER-FR.md CHANGELOG.md && git commit -S -m "docs: describe the correlation import, its read route and its screen" && git add docs/img/hub tests/browser/demo/fixtures/hub-embed.json && git commit -S -m "docs: refresh the launcher screenshots"
```

Then `graphify update .`, push the branch and open the PR into `main`. The PR description lists, in short lines: the two routes and the table, the Correlations screen and the incident block, the rulings this plan took (access on both sides, the range select, `ended_by`, Explain through Trace ID mode), and the website follow-up for `hub-embed.json` (strip the query of `/api/correlations` when replaying it).

---

### Task 11: Lab scenario `hub-correlations-push`

**Repository:** `/Users/robintrassard/IdeaProjects/perf-sentinel-simulation-lab`, on `main`.

**Files:**
- Create: `scenarios/hub-correlations-push/verify.sh` (executable)
- Create: `scenarios/hub-correlations-push/README.md`
- Modify: `Makefile` (`.PHONY` near line 59, the `validate` `bash -n` list near line 163, a target after `verify-hub-incidents-push` near line 637, the `verify-all-scenarios` list near line 825, the scenario count near line 768)
- Modify: `docs/SCENARIOS.md` (counts at lines 9, 198, 368, 433, 1464, the Hub coverage paragraph near line 352, a section after the `hub-incidents-mirror` one)

**Interfaces:**
- Consumes: a daemon image built from the engine branch carrying the correlation push (perf-sentinel 0.30.0) and a Hub image built from `feat/correlation-import`, seeded with `make seed-daemon-local` and `make seed-hub-local`. The Hub API of Tasks 5 and 6. `tools/tracegen/emit_at.py`. The lab keys `.perf-sentinel-incidents.key` and `.perf-sentinel-read.key`.
- Produces: `make verify-hub-correlations-push`, a report at `/tmp/scenario-hub-correlations-push-report.md`.

- [ ] **Step 1: Write the scenario**

Create `scenarios/hub-correlations-push/verify.sh`:

```bash
#!/usr/bin/env bash
# hub-correlations-push: a daemon pushes the periods a correlated pair was
# active, as episodes, and the Hub keeps them after the daemon forgets them.
#
# perf-sentinel 0.30.0 cuts an episode each time a pair of findings starts
# following each other again and pushes it to PerfSentinelHub 0.6.0's
# POST /api/import/correlations when [daemon.hub_export] and
# [daemon.correlation] are both on. An episode closes once the pair has not
# co-occurred for its gap, min(window, 30 min), one minute here.
#
# Legs, one numbered step each, on an isolated daemon and Hub:
#
#   0. Pre-flight. Keys, images, emit_at.py, the isolated pair with a one-minute
#      correlation window and a Hub that polls once a day.
#   1. The pair fires: N+1 in corpush-order, then 1 s later N+1 in
#      corpush-payment, five rounds. The Hub holds one open episode.
#   2. A silence past the gap. The daemon closes the episode, the Hub has its end.
#   3. The pair fires again with an incident posted during it. While it runs,
#      active=true lists the second episode alone. The Hub then holds two
#      episodes of the pair, and the incident's window overlaps the second only.
#   4. Nothing was dropped on the way.
#
# The pair is synthetic, sent with tools/tracegen/emit_at.py as
# correlation-event-time sends it: the shop's fault endpoints only reach the
# shared daemon through the collector, never an isolated one.
#
# Re-runnable. Everything lives in the scenario's own namespace, which the trap
# deletes.

set -euo pipefail

SCENARIO="hub-correlations-push"
REPORT="/tmp/scenario-${SCENARIO}-report.md"
TMP_DIR="/tmp/${SCENARIO}"
LAB_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
EMIT="${LAB_ROOT}/tools/tracegen/emit_at.py"

NS="${OBS_NS:-observability}"
ISO_NS="${HUB_CORRELATIONS_PUSH_NS:-hub-correlations-push}"
ISO_HUB_PORT="${HUB_CORRELATIONS_PUSH_HUB_PORT:-18097}"
ISO_DAEMON_PORT="${HUB_CORRELATIONS_PUSH_DAEMON_PORT:-14352}"
SOURCE_ID="lab-corpush-daemon"
ENVIRONMENT="lab"
SOURCE_SERVICE="corpush-order"
TARGET_SERVICE="corpush-payment"
ALERT_NS="corpush"
# The incident freezes 20 s back, so its window opens well after the first
# episode ended: only the second can overlap it.
LOOKBACK_MS=20000
# The payment side follows the order side by 1 s in event time, inside the
# 1.5 s lag threshold, while the next round's order side comes 3 s after it,
# outside it, so the reverse pair never forms.
LAG_MS=1000
SEND_GAP_S=2
ROUNDS=5
ROUNDS_AGAIN=10
# Three rounds into the second burst the pair is open again on the Hub.
INCIDENT_AFTER_S=12
# One flush interval, a request, a commit.
PUSH_WAIT_S=20
# The one-minute gap, a tracker round and a push on top of it.
CLOSE_WAIT_S=180
TRACE_BASE=$((0x5000))
TRACE_NUM=0

rm -rf "${TMP_DIR}"; mkdir -p "${TMP_DIR}"

color_blue()  { printf "\033[34m%s\033[0m\n" "$*"; }
color_green() { printf "\033[32m%s\033[0m\n" "$*"; }
color_red()   { printf "\033[31m%s\033[0m\n" "$*"; }
step() { color_blue "==> $*"; }
ok()   { color_green "    ok: $*"; }
fail() { color_red   "    fail: $*"; }
die()  { color_red   "    error: $*"; exit 1; }

declare -a NAMES=() VERDICTS=() NOTES=()
record() { NAMES+=("$1"); VERDICTS+=("$2"); NOTES+=("$3"); }

declare -a PF_PIDS=()
BURST_PID=""
cleanup() {
  [ -n "${BURST_PID}" ] && kill "${BURST_PID}" 2>/dev/null || true
  for pid in ${PF_PIDS[@]+"${PF_PIDS[@]}"}; do
    kill "${pid}" 2>/dev/null || true
  done
  if [ "${KEEP_NAMESPACE:-no}" != "yes" ]; then
    kubectl delete namespace "${ISO_NS}" --ignore-not-found --wait=false >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

jqp() { python3 -c "import json,sys; d=json.load(sys.stdin); print($1)"; }
now_ms() { python3 -c 'import time; print(int(time.time() * 1000))'; }

# One Alertmanager delivery. $1 = service, $2 = status, $3 = startsAt, $4 = endsAt.
alert_body() {
  python3 -c "
import json, sys
service, status, starts, ends = sys.argv[1:5]
print(json.dumps({'version': '4', 'alerts': [{
    'status': status,
    'labels': {'service': service, 'perf_sentinel_kind': 'oom_kill', 'namespace': '${ALERT_NS}'},
    'annotations': {'summary': 'correlation push ' + service},
    'startsAt': starts,
    'endsAt': ends,
}]}))
" "$1" "$2" "$3" "$4"
}

# $1 = minutes ago, as RFC 3339 in UTC.
minutes_ago() {
  python3 -c "
import datetime, sys
t = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(minutes=int(sys.argv[1]))
print(t.strftime('%Y-%m-%dT%H:%M:%SZ'))
" "$1"
}

# $1 = method, $2 = path, $3 = body ("" for none), $4 = key ("" for none).
# Prints the HTTP code, leaves the body in ${TMP_DIR}/body.
daemon_api() {
  local args=(-s -o "${TMP_DIR}/body" -w '%{http_code}' -X "$1")
  [ -n "${3:-}" ] && args+=(-H "content-type: application/json" -d "$3")
  [ -n "${4:-}" ] && args+=(-H "X-API-Key: $4")
  curl "${args[@]}" "http://127.0.0.1:${ISO_DAEMON_PORT}$2"
}

hub_api() {
  curl -s -o "${TMP_DIR}/body" -w '%{http_code}' -X "$1" "http://127.0.0.1:${ISO_HUB_PORT}$2"
}

# The Hub's episodes of this source for the query $1, into ${TMP_DIR}/body.
episodes() { hub_api GET "/api/correlations?source_id=${SOURCE_ID}&$1" >/dev/null; }

# One N+1 trace of service $1 on table $2, stamped at $3 (epoch ms), in its own batch.
emit() {
  TRACE_NUM=$((TRACE_NUM + 1))
  python3 "${EMIT}" --endpoint "http://127.0.0.1:${ISO_DAEMON_PORT}" --service "$1" --shape n_plus_one \
    --table "$2" --at-ms "$3" --trace-num "$((TRACE_BASE + TRACE_NUM))" \
    >> "${TMP_DIR}/send.log" 2>> "${TMP_DIR}/send.err" || echo "$1" >> "${TMP_DIR}/send.failed"
}

# $1 rounds of the pair: the order side now, the payment side LAG_MS later in
# event time, each send in its own batch.
burst() {
  local round t
  for round in $(seq 1 "$1"); do
    t="$(now_ms)"
    emit "${SOURCE_SERVICE}" orders "${t}"
    sleep "${SEND_GAP_S}"
    emit "${TARGET_SERVICE}" payments "$((t + LAG_MS))"
    sleep "${SEND_GAP_S}"
  done
}

no_send_failed() {
  [ ! -s "${TMP_DIR}/send.failed" ] \
    || die "$(wc -l < "${TMP_DIR}/send.failed") send(s) failed, see ${TMP_DIR}/send.err: the legs would grade an incomplete pair"
}

step "0. Pre-flight"
command -v kubectl >/dev/null || die "kubectl not on PATH"
command -v curl >/dev/null || die "curl not on PATH"
command -v python3 >/dev/null || die "python3 not on PATH"
python3 -c 'import opentelemetry.proto' 2>/dev/null \
  || die "emit_at.py needs opentelemetry-proto: pip install -r ${LAB_ROOT}/tools/tracegen/requirements.txt"

KEY_DIR="${HUB_CORRELATIONS_KEY_DIR:-${LAB_ROOT}}"
WRITE_KEY_FILE="${KEY_DIR}/.perf-sentinel-incidents.key"
READ_KEY_FILE="${KEY_DIR}/.perf-sentinel-read.key"
[ -f "${WRITE_KEY_FILE}" ] || die "no incidents write key at ${WRITE_KEY_FILE}. Run: make up"
[ -f "${READ_KEY_FILE}" ] || die "no read key at ${READ_KEY_FILE}. Run: make up"
WRITE_KEY="$(cat "${WRITE_KEY_FILE}")"
READ_KEY="$(cat "${READ_KEY_FILE}")"
# The import key is the scenario's own, never stored: it lives as long as the namespace.
IMPORT_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(24))')"

# The images under test are the ones the shared pair runs, seeded from the two
# branches with make seed-daemon-local and make seed-hub-local.
DAEMON_IMAGE="${HUB_CORRELATIONS_PUSH_DAEMON_IMAGE:-$(kubectl -n "${NS}" get deploy/perf-sentinel-daemon -o jsonpath='{.spec.template.spec.containers[0].image}')}"
HUB_IMAGE="${HUB_CORRELATIONS_PUSH_HUB_IMAGE:-$(kubectl -n "${NS}" get deploy/perf-sentinel-hub -o jsonpath='{.spec.template.spec.containers[0].image}')}"
[ -n "${DAEMON_IMAGE}" ] && [ -n "${HUB_IMAGE}" ] || die "no daemon or Hub image to run. Run: make seed-daemon-local and make seed-hub-local"

for _ in $(seq 1 60); do
  [ "$(kubectl get namespace "${ISO_NS}" -o jsonpath='{.status.phase}' 2>/dev/null)" = "Terminating" ] || break
  sleep 2
done
kubectl create namespace "${ISO_NS}" --dry-run=client -o yaml | kubectl apply -f - >/dev/null
kubectl -n "${ISO_NS}" create secret generic api-keys \
  --from-literal=incidents-api-key="${WRITE_KEY}" --from-literal=read-api-key="${READ_KEY}" \
  --from-literal=import-api-key="${IMPORT_KEY}" \
  --dry-run=client -o yaml | kubectl apply -f - >/dev/null

kubectl apply -f - >/dev/null <<EOF
---
apiVersion: v1
kind: ConfigMap
metadata: {name: daemon-config, namespace: ${ISO_NS}}
data:
  config.toml: |
    [daemon]
    listen_address = "0.0.0.0"
    listen_port_http = 14318
    listen_port_grpc = 14317
    api_enabled = true
    environment = "staging"
    # One trace per batch: a short TTL closes each trace before the next send.
    trace_ttl_ms = 1000

    [daemon.ack]
    enabled = false

    [daemon.incidents]
    enabled = true
    lookback_ms = ${LOOKBACK_MS}

    # A one-minute window makes the episode gap one minute.
    [daemon.correlation]
    enabled = true
    window_minutes = 1
    lag_threshold_ms = 1500
    min_co_occurrences = 2
    min_confidence = 0.5

    [detection]
    n_plus_one_min_occurrences = 5

    [daemon.hub_export]
    enabled = true
    endpoint = "http://hub.${ISO_NS}.svc.cluster.local:8080/api/import/findings"
    source_id = "${SOURCE_ID}"
    api_key_file = "/etc/perf-sentinel-hub/import.key"
    flush_interval_secs = 1
---
apiVersion: apps/v1
kind: Deployment
metadata: {name: daemon, namespace: ${ISO_NS}}
spec:
  replicas: 1
  strategy: {type: Recreate}
  selector: {matchLabels: {app: daemon}}
  template:
    metadata: {labels: {app: daemon}}
    spec:
      automountServiceAccountToken: false
      securityContext:
        runAsNonRoot: true
        runAsUser: 65534
        runAsGroup: 65534
        seccompProfile: {type: RuntimeDefault}
      containers:
        - name: perf-sentinel
          image: ${DAEMON_IMAGE}
          imagePullPolicy: Never
          args: [watch, --config, /etc/perf-sentinel/config.toml]
          ports: [{name: http, containerPort: 14318}]
          env:
            - name: PERF_SENTINEL_INCIDENTS_API_KEY
              valueFrom: {secretKeyRef: {name: api-keys, key: incidents-api-key}}
            - name: PERF_SENTINEL_READ_API_KEY
              valueFrom: {secretKeyRef: {name: api-keys, key: read-api-key}}
          volumeMounts:
            - {name: config, mountPath: /etc/perf-sentinel, readOnly: true}
            - {name: import-key, mountPath: /etc/perf-sentinel-hub, readOnly: true}
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities: {drop: [ALL]}
          readinessProbe:
            httpGet: {path: /health, port: http}
            periodSeconds: 2
      volumes:
        - name: config
          configMap: {name: daemon-config}
        - name: import-key
          secret:
            secretName: api-keys
            items: [{key: import-api-key, path: import.key}]
---
apiVersion: v1
kind: Service
metadata: {name: daemon, namespace: ${ISO_NS}}
spec:
  selector: {app: daemon}
  ports: [{name: http, port: 14318, targetPort: http}]
---
apiVersion: v1
kind: ConfigMap
metadata: {name: hub-config, namespace: ${ISO_NS}}
data:
  appsettings.Production.json: |
    {"Hub": {
      "DatabasePath": "/data/hub.db",
      "PollInterval": "1.00:00:00",
      "HttpTimeout": "00:00:05",
      "Retention": "180.00:00:00",
      "ResolutionGrace": "01:00:00",
      "MaxReadLimit": 10000,
      "Sources": [{
        "Id": "${SOURCE_ID}", "Name": "correlation push lab daemon", "Environment": "${ENVIRONMENT}",
        "BaseUrl": "http://daemon.${ISO_NS}.svc.cluster.local:14318",
        "AuthHeaderName": "X-API-Key"
      }]
    }}
---
apiVersion: apps/v1
kind: Deployment
metadata: {name: hub, namespace: ${ISO_NS}}
spec:
  replicas: 1
  strategy: {type: Recreate}
  selector: {matchLabels: {app: hub}}
  template:
    metadata: {labels: {app: hub}}
    spec:
      automountServiceAccountToken: false
      securityContext:
        runAsNonRoot: true
        runAsUser: 1654
        runAsGroup: 1654
        fsGroup: 1654
        seccompProfile: {type: RuntimeDefault}
      containers:
        - name: hub
          image: ${HUB_IMAGE}
          imagePullPolicy: Never
          ports: [{name: http, containerPort: 8080}]
          env:
            - name: ASPNETCORE_ENVIRONMENT
              value: Production
            - name: Hub__Sources__0__AuthHeaderValue
              valueFrom: {secretKeyRef: {name: api-keys, key: read-api-key}}
            - name: Hub__Sources__0__ImportApiKey
              valueFrom: {secretKeyRef: {name: api-keys, key: import-api-key}}
          volumeMounts:
            - {name: config, mountPath: /app/appsettings.Production.json, subPath: appsettings.Production.json, readOnly: true}
            - {name: data, mountPath: /data}
            - {name: tmp, mountPath: /tmp}
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities: {drop: [ALL]}
          readinessProbe:
            httpGet: {path: /health/ready, port: http}
            periodSeconds: 2
      volumes:
        - name: config
          configMap: {name: hub-config}
        - name: data
          emptyDir: {}
        - name: tmp
          emptyDir: {}
---
apiVersion: v1
kind: Service
metadata: {name: hub, namespace: ${ISO_NS}}
spec:
  selector: {app: hub}
  ports: [{name: http, port: 8080, targetPort: http}]
EOF
# The Hub first, so the daemon's first pushes find it answering.
kubectl -n "${ISO_NS}" rollout status deploy/hub --timeout=180s >/dev/null \
  || die "the isolated Hub did not become ready: $(kubectl -n "${ISO_NS}" logs deploy/hub --tail=20 2>&1)"
kubectl -n "${ISO_NS}" rollout status deploy/daemon --timeout=180s >/dev/null \
  || die "the isolated daemon did not become ready: $(kubectl -n "${ISO_NS}" logs deploy/daemon --tail=20 2>&1)"

iso_forward() {  # $1 = service, $2 = local port, $3 = remote port, $4 = probe path
  local i pid
  kubectl -n "${ISO_NS}" port-forward "svc/$1" "$2:$3" >"${TMP_DIR}/pf-$1.log" 2>&1 &
  pid="$!"
  PF_PIDS+=("${pid}")
  disown "${pid}" 2>/dev/null || true
  for i in $(seq 1 40); do
    curl -sf "http://127.0.0.1:$2$4" >/dev/null 2>&1 && return 0
    sleep 0.5
  done
  return 1
}
iso_forward hub "${ISO_HUB_PORT}" 8080 /health/ready || die "the isolated Hub never answered on ${ISO_HUB_PORT}"
iso_forward daemon "${ISO_DAEMON_PORT}" 14318 /health || die "the isolated daemon never answered on ${ISO_DAEMON_PORT}"

# A Hub without the route, or a daemon without the push, would fail leg 1 for the wrong reason.
ROUTE_CODE="$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:${ISO_HUB_PORT}/api/import/correlations?source_id=${SOURCE_ID}")"
[ "${ROUTE_CODE}" = "401" ] \
  || die "the Hub answered HTTP ${ROUTE_CODE} on POST /api/import/correlations without a key, 401 expected. Its image ${HUB_IMAGE} predates the route"
curl -s "http://127.0.0.1:${ISO_DAEMON_PORT}/metrics" | grep -q '^perf_sentinel_hub_export_correlations_pending ' \
  || die "the daemon publishes no perf_sentinel_hub_export_correlations_pending. Its image ${DAEMON_IMAGE} predates the correlation push"
ok "isolated pair in ${ISO_NS}: daemon ${DAEMON_IMAGE}, Hub ${HUB_IMAGE}, one-minute window, poll once a day"

step "1. The pair fires and the Hub holds an open episode"
burst "${ROUNDS}"
no_send_failed
FIRST=""
for _ in $(seq 1 "${PUSH_WAIT_S}"); do
  episodes "service=${SOURCE_SERVICE}"
  FIRST="$(jqp "' '.join([d[0]['id'], d[0]['pair_key'], d[0]['source']['finding_type'] + '>' + d[0]['target']['service']]) if len(d) == 1 and d[0].get('ended_at_ms') is None else ''" < "${TMP_DIR}/body")"
  [ -n "${FIRST}" ] && break
  sleep 1
done
read -r FIRST_ID PAIR_KEY FIRST_PAIR <<< "${FIRST:-- - -}"
if [ -n "${FIRST}" ] && [ "${FIRST_PAIR}" = "n_plus_one_sql>${TARGET_SERVICE}" ]; then
  ok "one open episode ${FIRST_ID}: ${SOURCE_SERVICE} then ${TARGET_SERVICE}"
  record "an active pair reaches the Hub" PASS "episode ${FIRST_ID}, pair ${PAIR_KEY}"
else
  fail "no single open episode of ${SOURCE_SERVICE} then ${TARGET_SERVICE} on the Hub"
  record "an active pair reaches the Hub" FAIL "$(head -c 200 "${TMP_DIR}/body")"
  die "no episode to follow, the later legs would grade nothing"
fi

step "2. A silence past the gap closes it"
# Closed by the daemon: an end, and no ended_by, which only the Hub's stale rule sets.
STATE="missing"
for _ in $(seq 1 "${CLOSE_WAIT_S}"); do
  episodes "pair_key=${PAIR_KEY}"
  STATE="$(jqp "next((('stale' if r.get('ended_by') == 'stale' else 'closed' if r.get('ended_at_ms') is not None else 'open') for r in d if r['id'] == '${FIRST_ID}'), 'missing')" < "${TMP_DIR}/body")"
  [ "${STATE}" = "closed" ] && break
  sleep 1
done
if [ "${STATE}" = "closed" ]; then
  ok "the daemon closed ${FIRST_ID} and the Hub carries the end"
  record "the daemon closes the episode after the gap" PASS "ended_at_ms set, ended_by null"
else
  fail "${FIRST_ID} still reads '${STATE}' after ${CLOSE_WAIT_S} s"
  record "the daemon closes the episode after the gap" FAIL "state ${STATE}"
fi

step "3. The pair fires again, an incident posted during it"
burst "${ROUNDS_AGAIN}" &
BURST_PID="$!"
sleep "${INCIDENT_AFTER_S}"
CODE="$(daemon_api POST /api/incidents "$(alert_body "${SOURCE_SERVICE}" firing "$(minutes_ago 0)" "0001-01-01T00:00:00Z")" "${WRITE_KEY}")"
[ "${CODE}" = "200" ] || die "the daemon refused the alert (HTTP ${CODE}): $(head -c 200 "${TMP_DIR}/body")"
ACTIVE=""
for _ in $(seq 1 "${PUSH_WAIT_S}"); do
  episodes "active=true"
  ACTIVE="$(jqp "','.join(r['id'] for r in d)" < "${TMP_DIR}/body")"
  [ -n "${ACTIVE}" ] && [ "${ACTIVE}" != "${FIRST_ID}" ] && break
  sleep 1
done
SECOND_ID=""
if [ -n "${ACTIVE}" ] && [ "${ACTIVE}" = "${ACTIVE%%,*}" ] && [ "${ACTIVE}" != "${FIRST_ID}" ]; then
  SECOND_ID="${ACTIVE}"
  ok "while it runs, active=true lists ${SECOND_ID} alone"
  record "the second episode is the only active one" PASS "active=true: ${SECOND_ID}"
else
  fail "active=true lists '${ACTIVE}'"
  record "the second episode is the only active one" FAIL "active=true: '${ACTIVE}'"
fi
wait "${BURST_PID}" || true
BURST_PID=""
no_send_failed

episodes "pair_key=${PAIR_KEY}"
TWO="$(jqp "len(d) == 2 and d[0]['id'] == '${SECOND_ID}' and d[1]['id'] == '${FIRST_ID}' and d[1].get('ended_at_ms') is not None and d[0]['started_at_ms'] > d[1]['ended_at_ms']" < "${TMP_DIR}/body")"
if [ "${TWO}" = "True" ]; then
  ok "two episodes of the pair, the second started after the first ended"
  record "a pair active twice is two episodes" PASS "${FIRST_ID} then ${SECOND_ID}"
else
  fail "the pair's episodes are not the two expected: $(head -c 300 "${TMP_DIR}/body")"
  record "a pair active twice is two episodes" FAIL "$(jqp "len(d)" < "${TMP_DIR}/body") episode(s)"
fi

WINDOW=""
for _ in $(seq 1 "${PUSH_WAIT_S}"); do
  hub_api GET "/api/incidents?source_id=${SOURCE_ID}&service=${SOURCE_SERVICE}" >/dev/null
  WINDOW="$(jqp "'%d %d' % (d[0]['window_from_ms'], d[0]['ended_at_ms'] or d[0]['window_to_ms']) if len(d) == 1 else ''" < "${TMP_DIR}/body")"
  [ -n "${WINDOW}" ] && break
  sleep 1
done
[ -n "${WINDOW}" ] || die "the incident never reached the Hub, the overlap leg has no window"
read -r WIN_FROM WIN_TO <<< "${WINDOW}"
episodes "from=${WIN_FROM}&to=${WIN_TO}"
OVERLAP="$(jqp "','.join(r['id'] for r in d)" < "${TMP_DIR}/body")"
if [ -n "${SECOND_ID}" ] && [ "${OVERLAP}" = "${SECOND_ID}" ]; then
  ok "the incident's window [${WIN_FROM}, ${WIN_TO}] overlaps the second episode alone"
  record "the incident overlaps the second episode only" PASS "window ${WIN_FROM}-${WIN_TO}"
else
  fail "the incident's window overlaps '${OVERLAP}', ${SECOND_ID} expected"
  record "the incident overlaps the second episode only" FAIL "overlap '${OVERLAP}'"
fi

step "4. Nothing was dropped"
DROPPED=""; PENDING=""
for _ in $(seq 1 10); do
  curl -s "http://127.0.0.1:${ISO_DAEMON_PORT}/metrics" > "${TMP_DIR}/metrics"
  DROPPED="$(awk '/^perf_sentinel_hub_export_correlations_dropped_total /{print $2}' "${TMP_DIR}/metrics")"
  PENDING="$(awk '/^perf_sentinel_hub_export_correlations_pending /{print $2}' "${TMP_DIR}/metrics")"
  [ "${PENDING}" = "0" ] && break
  sleep 1
done
if [ "${DROPPED}" = "0" ] && [ "${PENDING}" = "0" ]; then
  ok "no episode dropped, none left pending"
  record "nothing dropped" PASS "dropped ${DROPPED}, pending ${PENDING}"
else
  fail "dropped '${DROPPED}', pending '${PENDING}'"
  record "nothing dropped" FAIL "dropped '${DROPPED}', pending '${PENDING}'"
fi

# === Report ===
{
  echo "# Scenario: ${SCENARIO}"
  echo
  echo "Daemon: \`${DAEMON_IMAGE}\`"
  echo
  echo "Hub: \`${HUB_IMAGE}\`"
  echo
  echo "Isolated pair in namespace \`${ISO_NS}\`, source \`${SOURCE_ID}\`, one-minute correlation window, Hub poll once a day."
  echo
  echo "| Sub-test | Verdict | Note |"
  echo "| --- | --- | --- |"
  for i in "${!NAMES[@]}"; do
    echo "| ${NAMES[$i]} | ${VERDICTS[$i]} | ${NOTES[$i]} |"
  done
} > "${REPORT}"

step "Report written to ${REPORT}"
for v in "${VERDICTS[@]}"; do
  [ "${v}" = "PASS" ] || { color_red "SCENARIO FAILED"; exit 1; }
done
color_green "SCENARIO PASSED"
```

Then `chmod +x scenarios/hub-correlations-push/verify.sh`.

- [ ] **Step 2: Lint it, then watch it fail on the current images**

Run: `bash -n scenarios/hub-correlations-push/verify.sh && ./scenarios/hub-correlations-push/verify.sh`
Expected: `bash -n` silent, then the script stops in pre-flight with "the Hub answered HTTP 404 on POST /api/import/correlations without a key, 401 expected" against the published 0.5.0 Hub the shared pair runs.

- [ ] **Step 3: Write the README**

Create `scenarios/hub-correlations-push/README.md`:

```markdown
# hub-correlations-push

**Claim.** A daemon on perf-sentinel 0.30.0 or later with `[daemon.hub_export]` and
`[daemon.correlation]` both on pushes each period a correlated pair stays active, as an
episode, to the Hub's `POST /api/import/correlations` (PerfSentinelHub 0.6.0 or later). The
daemon closes an episode once the pair has not co-occurred for its gap, so a pair that fires,
falls silent and fires again is two episodes on the Hub, and an incident posted during the
second overlaps it alone.

## The pair

No shop fault endpoint reaches an isolated daemon: the collector exports to the shared daemon
only. The scenario sends the pair `correlation-event-time` already proved, with
`tools/tracegen/emit_at.py --shape n_plus_one`: an N+1 SQL trace in `corpush-order` on table
`orders`, then 1 s later in event time an N+1 SQL trace in `corpush-payment` on table
`payments`, each in its own batch. The pair the daemon reports is
`n_plus_one_sql/corpush-order` then `n_plus_one_sql/corpush-payment`.

## Setup

An isolated daemon and Hub in the scenario's own namespace, from the images the shared pair
runs (seed them with `make seed-daemon-local` and `make seed-hub-local` from the two branches
under test):

- the daemon runs `[daemon.correlation]` with a one-minute window, so the episode gap is one
  minute, freezes incidents 20 s back, and pushes to the Hub with a scenario-generated import
  key, flushing every second;
- the Hub polls once a day, so everything it holds came by push.

The pre-flight refuses a Hub that answers anything but 401 on the correlation route without a
key, and a daemon that publishes no `perf_sentinel_hub_export_correlations_pending`, since
either would fail leg 1 for the wrong reason. It also needs the Python packages of
`tools/tracegen/requirements.txt`.

## Verdicts

| Sub-test | Passes when |
| --- | --- |
| an active pair reaches the Hub | five rounds leave one open episode of the pair on the Hub |
| the daemon closes the episode after the gap | the episode carries an `ended_at_ms` and no `ended_by` within three minutes of silence |
| the second episode is the only active one | while the pair fires again, `active=true` lists the new episode alone |
| a pair active twice is two episodes | the pair's listing holds the two, the second started after the first ended |
| the incident overlaps the second episode only | the incident's window, read back from the Hub, overlaps the second episode and not the first |
| nothing dropped | `perf_sentinel_hub_export_correlations_dropped_total` and `..._pending` are both 0 |

The report lands in `/tmp/scenario-hub-correlations-push-report.md`. `KEEP_NAMESPACE=yes`
keeps the isolated pair for inspection.
```

- [ ] **Step 4: Wire it into the Makefile and the guide**

`Makefile`:

- `.PHONY`: `verify-hub-incidents-push \` becomes `verify-hub-incidents-push verify-hub-correlations-push \`;
- `validate`: after `@bash -n scenarios/hub-incidents-push/verify.sh`, add `	@bash -n scenarios/hub-correlations-push/verify.sh`;
- after the `verify-hub-incidents-push` target:

```make
verify-hub-correlations-push: ## daemon pushes correlation episodes: a pair active twice around a silence lands as two, the second overlapping an incident
	./scenarios/hub-correlations-push/verify.sh
```

- in the `verify-all-scenarios` loop, `hub-incidents-push` becomes `hub-incidents-push hub-correlations-push`;
- `## Run all 95 scenarios sequentially` becomes `## Run all 100 scenarios sequentially`.

`docs/SCENARIOS.md`: replace `95` with `100` at the five places that count scenarios (lines 9, 198, 368, 433, 1464, check each with `grep -n "95" docs/SCENARIOS.md` and leave the `p95` lines alone). In the Hub coverage paragraph, after the sentence on `hub-incidents-mirror`, add:

```markdown
`hub-correlations-push` is the 0.30.0 addition, the correlation history the Hub keeps once the
daemon forgets it: a pair active twice around a silence longer than its gap lands as two
episodes, and an incident posted during the second overlaps it alone. It has its own section
too.
```

After the `hub-incidents-mirror` section (before the next `## ` heading), add:

```markdown
## hub-correlations-push (0.30.0 correlation episodes pushed to the Hub)

`make verify-hub-correlations-push`. Needs the cluster, `make seed-daemon-local` and
`make seed-hub-local` from the branches under test, the two API keys `scripts/bootstrap.sh`
generates, and the Python packages of `tools/tracegen/requirements.txt`. About five minutes,
most of it the one-minute gap the daemon waits out before it closes an episode.

The daemon's correlator keeps a pair only while its counters cover the rolling window, so a
correlation active this morning is gone by noon. The engine's tests prove the tracker cuts
episodes, the Hub's tests prove it stores and merges them, and neither sees the other. This
scenario is the seam: one isolated daemon and Hub, as in `hub-incidents-push`, a pair sent with
`emit_at.py` (no shop endpoint reaches an isolated daemon), a silence past the gap, the pair
again with an incident posted during it.

**The daemon cuts, the Hub keeps.** The first burst must leave one open episode on the Hub,
named after the side that fires first. The silence must give it an `ended_at_ms` with no
`ended_by`, an end pushed by the daemon rather than inferred by the Hub's stale rule, which waits
fifteen minutes more and would set `ended_by: stale`.

**Active means the running one.** While the second burst runs, `active=true` must list the new
episode and not the closed one: that is the read the launcher's "active now only" switch and a
Grafana panel make.

**An incident finds the episode it overlaps.** The incident freezes 20 s back, so its window
opens long after the first episode ended. Read back from the Hub, that window must overlap the
second episode alone, the same read an incident's detail makes in the launcher.
```

- [ ] **Step 5: Run it against the branches under test**

```bash
cd /Users/robintrassard/IdeaProjects/perf-sentinel-simulation-lab && make validate && make seed-daemon-local && make seed-hub-local && make verify-hub-correlations-push
```

The engine checkout must sit on the branch carrying the correlation push (perf-sentinel 0.30.0) and the Hub checkout on `feat/correlation-import`.
Expected: `make validate` exit 0, then "SCENARIO PASSED" with six PASS rows in `/tmp/scenario-hub-correlations-push-report.md`.

- [ ] **Step 6: Commit on main, without the pins**

```bash
cd /Users/robintrassard/IdeaProjects/perf-sentinel-simulation-lab && git add scenarios/hub-correlations-push/verify.sh scenarios/hub-correlations-push/README.md Makefile docs/SCENARIOS.md && git diff --cached --name-only | grep -q '^manifests/' && echo "manifest pin staged, unstage it" || git commit -S -m "feat(hub-correlations-push): a pair active twice lands as two episodes on the Hub"
```

`git status` must still list `manifests/perf-sentinel-daemon.yaml` and `manifests/perf-sentinel-hub.yaml` as modified and unstaged. Leave them so until the release repins the published images, then `git checkout -- manifests/`.

---

## Self-review against the spec

- **Storage:** table, columns, key, three indexes and migration in Task 2, merge rule in Task 2 with the tie-break of Ruling 5, purge on `last_seen_ms` in Task 2.
- **Import route:** admission, key, gate, 1 MiB, 1 to 100, duplicates, per-episode checks, 400 when none accepted, 5 s write, 200 body, metric, `source_imports`, reachability untouched: Tasks 1, 4 and 5.
- **Read route:** scoping, every filter, order, limits, stale rule: Tasks 3 and 6.
- **Launcher:** tab, heading, filters, pair rows, cut line, unfolded strip with incidents, episodes with incident links, Analyse, Explain, incident block with both empty states, phone layout: Tasks 7, 8 and 9. Screenshots and demo fixtures: Tasks 8 and 10.
- **Documentation:** API, OPERATIONS, ARCHITECTURE, the three route inventories, CHANGELOG, screenshots, with FR mirrors: Task 10.
- **Testing:** parser, merge in both orders, admission, filters and overlap, scoping, stale rule, purge, Playwright in both themes and at 390 px, lab scenario: Tasks 1 to 11.
- **Engine plan rulings 1 to 4:** grouping value parsed and stored (Tasks 1 and 2), bare array with `source_name` and stale-only `ended_by` (Tasks 3 and 6), blank filters (Task 6), `#/correlations?pair_key=` route (Tasks 7 to 9), the lab reading an end without `ended_by` as a daemon close (Task 11).
- **Not in this plan, by the brief:** the engine side, the Grafana panel, the 0.6.0 release, the website's replay of `/api/correlations` (Ruling 11).
