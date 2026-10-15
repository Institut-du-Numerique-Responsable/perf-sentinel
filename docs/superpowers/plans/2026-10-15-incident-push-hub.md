# Incident push, Hub side, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** PerfSentinelHub accepts daemon incidents pushed to `POST /api/import/incidents?source_id=<id>` and stores them exactly as the poll does.

**Architecture:** A second import route beside `POST /api/import/findings`, sharing its admission (query check, `X-API-Key` against the source's `ImportApiKey`, the 4-slot import gate). The body is `{producer_version, incidents}` with each incident in the daemon's `GET /api/incidents` shape, parsed by the poll's `IncidentParser` and written through the poll's incident upsert, behind a write-gate wait bounded to 5 s. Refusals count on a new label-stable metric.

**Tech Stack:** .NET 10 NativeAOT minimal API, SQLite (Microsoft.Data.Sqlite), xunit v3 with `HubApplicationFactory`.

**Spec:** `perf-sentinel/docs/superpowers/specs/2026-10-15-incident-push-to-hub-design.md` (engine repo).

**Repository:** `/Users/robintrassard/RiderProjects/PerfSentinelHub`. Branch `feat/incident-import` cut from `main` once PR 93 is merged. Pinned SDK on PATH: `export DOTNET_ROOT=~/.local/share/dotnet-10.0.401 PATH=$DOTNET_ROOT:$PATH`.

## Global Constraints

- Route: `POST /api/import/incidents?source_id=<id>`, `.AllowAnonymous()`, same query validation as the findings import (valid UTF-8, exactly one `source_id`, non-empty).
- Auth: `X-API-Key` compared to `SourceOptions.ImportApiKey` with the existing `IsAuthorized` (SHA-256 then `FixedTimeEquals`). Unknown source or wrong key: 401.
- Gate: the existing `ImportGate` (4 slots) shared with the findings import. Full: 503 with `Retry-After: 1`.
- Body cap: 4 MiB (`4 * 1024 * 1024`), else 413.
- Envelope: `{"producer_version": "<1..64 chars>", "incidents": [1..10 incidents]}`, duplicate properties refused (`JsonRead.UniqueProperties`). More than 10: 413. Zero, or none accepted: 400.
- Write: wait at most 5 s (`WriteGateWait`) for the write gate, else 503 `Retry-After: 1`. Merge rule unchanged (richer copy wins, end kept once seen). Upsert `source_imports.last_import_ms`. Never touch `incident_reads` nor `source_state`.
- Response: 200 `{"accepted": n, "rejected": m}` (the existing `ImportResponse`) after the commit.
- Metric: `perf_sentinel_hub_incident_import_rejected_total{reason}` with reasons `bad_request`, `unauthorized`, `gate_full`, `write_timeout`, `too_large`, all published from startup. `perf_sentinel_hub_import_rejected_total` is unchanged.
- Log: event 1302 at Warning, "Source {SourceId} rejected {RejectedCount} imported incidents.", declared in `ApiEndpoints.cs` beside 1301 (`LoggerMessageLayoutTests` requires it).
- English only in code, comments, docs; every `docs/X.md` change ships with `docs/FR/X-FR.md` in the same commit. No em dashes, no semicolons in prose, straight quotes.
- Commits: Conventional Commits, subject line only, signed (`-S`), no Co-Authored-By.
- Sonar: cognitive complexity per method at most 15, no duplicated blocks between the two import handlers (share the admission prelude).

## Review Focus

- A push and a poll of the same incident in either order must leave one row whose `finding_count`, `findings_json` and `ended_at_ms` match the richer copy. Pinned in Task 2.
- A pushed incident whose findings array is shorter than the stored one (a daemon restarted and re-froze less) must not shrink the stored copy, but its `ended_at_ms` must still land. Pinned in Task 2.
- A body with 11 incidents must be refused 413 before any write, and one of exactly 4 MiB plus one byte must be 413 without buffering more. Pinned in Task 4.
- An incident with an invalid id (not 32 lowercase hex) next to a valid one: the valid one is stored, the response says `accepted: 1, rejected: 1`, and event 1302 is logged. Pinned in Task 4.
- A push must not mark the source reachable nor file an incidents read: `incident_reads` stays absent for a source that has only been pushed to. Pinned in Task 4.

---

### Task 1: Parse the incident import envelope

**Files:**
- Modify: `PerfSentinelHub/Collection/IncidentParser.cs`
- Test: `PerfSentinelHub.Tests/IncidentParserTests.cs`

**Interfaces:**
- Produces: `public sealed record ParsedIncidentImport(string ProducerVersion, ParsedIncidentPage Page);` and `public static ParsedIncidentImport IncidentParser.ParseImport(ReadOnlyMemory<byte> payload)`, which throws `InvalidDataException` for a malformed envelope or an empty `incidents`, and `ImportBatchTooLargeException` (from `FindingParser.cs`) for more than `IncidentParser.MaxImportIncidents` (10).

- [ ] **Step 1: Write the failing tests** (append to `IncidentParserTests`)

```csharp
    private static byte[] Envelope(string incidents, string producerVersion = "0.29.0")
    {
        return Encoding.UTF8.GetBytes($"{{\"producer_version\":\"{producerVersion}\",\"incidents\":{incidents}}}");
    }

    private static string FixtureIncidents()
    {
        return File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "Fixtures", "daemon-incidents-0.20.0.json"));
    }

    [Fact]
    public void An_import_envelope_parses_its_incidents_as_the_poll_does()
    {
        var import = IncidentParser.ParseImport(Envelope(FixtureIncidents()));

        Assert.Equal("0.29.0", import.ProducerVersion);
        var incident = Assert.Single(import.Page.Incidents);
        Assert.Equal("d650edad80ac5c2d99b8d1dde07100c2", incident.Id);
        Assert.Equal(2, incident.FindingCount);
        Assert.Equal(0, import.Page.RejectedCount);
    }

    [Theory]
    [InlineData("[]")]
    [InlineData("{}")]
    public void An_import_without_incidents_is_invalid(string incidents)
    {
        Assert.Throws<InvalidDataException>(() => IncidentParser.ParseImport(Envelope(incidents)));
    }

    [Fact]
    public void An_import_refuses_a_missing_or_overlong_producer_version()
    {
        Assert.Throws<InvalidDataException>(() => IncidentParser.ParseImport(
            Encoding.UTF8.GetBytes($"{{\"incidents\":{FixtureIncidents()}}}")));
        Assert.Throws<InvalidDataException>(() => IncidentParser.ParseImport(
            Envelope(FixtureIncidents(), new string('9', 65))));
    }

    [Fact]
    public void An_import_of_more_than_ten_incidents_is_too_large()
    {
        var one = FixtureIncidents().Trim()[1..^1];
        var eleven = "[" + string.Join(",", Enumerable.Repeat(one, 11)) + "]";

        Assert.Throws<ImportBatchTooLargeException>(() => IncidentParser.ParseImport(Envelope(eleven)));
    }

    [Fact]
    public void An_import_repeating_a_property_is_invalid()
    {
        var payload = Encoding.UTF8.GetBytes(
            $"{{\"producer_version\":\"0.29.0\",\"producer_version\":\"0.28.0\",\"incidents\":{FixtureIncidents()}}}");

        Assert.Throws<InvalidDataException>(() => IncidentParser.ParseImport(payload));
    }
```

Add `using System.Text;` at the top of the file if it is not there.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `dotnet test PerfSentinelHub.Tests --filter-class PerfSentinelHub.Tests.IncidentParserTests`
Expected: build error, `IncidentParser.ParseImport` does not exist.

- [ ] **Step 3: Implement**

In `IncidentParser.cs`, add the record next to `ParsedIncidentPage`:

```csharp
public sealed record ParsedIncidentImport(string ProducerVersion, ParsedIncidentPage Page);
```

Inside `IncidentParser`, split the array walk out of `Parse` and add `ParseImport`:

```csharp
    /// <summary>A pushed batch holds at most this many incidents, each up to a thousand findings.</summary>
    public const int MaxImportIncidents = 10;

    public static ParsedIncidentPage Parse(ReadOnlyMemory<byte> payload)
    {
        using var document = Open(payload, "The incidents response is not valid JSON.");
        if (document.RootElement.ValueKind != JsonValueKind.Array)
            throw new InvalidDataException("The incidents response must be a JSON array.");
        return ParseArray(document.RootElement);
    }

    /// <summary>
    ///     A daemon's push: its version beside incidents in the very shape its
    ///     listing serves, so a pushed copy and a polled one parse alike.
    /// </summary>
    public static ParsedIncidentImport ParseImport(ReadOnlyMemory<byte> payload)
    {
        using var document = Open(payload, "The import body is not valid JSON.");
        var root = document.RootElement;
        if (root.ValueKind != JsonValueKind.Object ||
            JsonRead.ReadString(root, "producer_version") is not { Length: > 0 and <= 64 } producerVersion ||
            !root.TryGetProperty("incidents", out var incidents) ||
            incidents.ValueKind != JsonValueKind.Array)
            throw new InvalidDataException("The import body is invalid.");

        var count = incidents.GetArrayLength();
        if (count > MaxImportIncidents)
            throw new ImportBatchTooLargeException();
        if (count == 0)
            throw new InvalidDataException("The import batch is empty.");
        return new ParsedIncidentImport(producerVersion, ParseArray(incidents));
    }

    private static JsonDocument Open(ReadOnlyMemory<byte> payload, string invalid)
    {
        try
        {
            return JsonDocument.Parse(payload, JsonRead.UniqueProperties);
        }
        catch (JsonException exception)
        {
            throw new InvalidDataException(invalid, exception);
        }
    }

    private static ParsedIncidentPage ParseArray(JsonElement array)
    {
        var incidents = new List<ParsedIncident>();
        var rejected = 0;
        foreach (var element in array.EnumerateArray())
            if (TryParse(element, out var incident))
                incidents.Add(incident);
            else
                rejected++;
        return new ParsedIncidentPage(incidents, rejected);
    }
```

Remove the old body of `Parse` (the `try`/`using (document)` block) that this replaces.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `dotnet test PerfSentinelHub.Tests --filter-class PerfSentinelHub.Tests.IncidentParserTests`
Expected: PASS, the existing parser tests included.

- [ ] **Step 5: Commit**

```bash
git add PerfSentinelHub/Collection/IncidentParser.cs PerfSentinelHub.Tests/IncidentParserTests.cs
git commit -S -m "feat(import): parse a pushed batch of daemon incidents"
```

---

### Task 2: Store pushed incidents behind a bounded write wait

**Files:**
- Modify: `PerfSentinelHub/Storage/HubDatabase.Incidents.cs`
- Modify: `PerfSentinelHub/Storage/HubDatabase.cs` (extract the `source_imports` upsert)
- Test: `PerfSentinelHub.Tests/IncidentImportStorageTests.cs` (create)

**Interfaces:**
- Consumes: `ParsedIncident` (Task 1 unchanged), `IncidentParser.Parse`.
- Produces: `public Task<bool> HubDatabase.TryUpsertPushedIncidentsAsync(string sourceId, IReadOnlyList<ParsedIncident> incidents, long observedAtMs, CancellationToken cancellationToken)`, false when the write gate stayed busy for 5 s.

- [ ] **Step 1: Write the failing tests**

Create `PerfSentinelHub.Tests/IncidentImportStorageTests.cs`:

```csharp
using System.Text;
using System.Text.Json.Nodes;
using PerfSentinelHub.Collection;

namespace PerfSentinelHub.Tests;

public sealed class IncidentImportStorageTests(HubApplicationFactory factory) : IClassFixture<HubApplicationFactory>
{
    private const string Source = "test";

    // The fixture's single incident, with its id, its end and its findings set by the test.
    private static ParsedIncident Incident(string id, long? endedAtMs, int findings)
    {
        var array = JsonNode.Parse(File.ReadAllText(Path.Combine(
            AppContext.BaseDirectory, "Fixtures", "daemon-incidents-0.20.0.json")))!.AsArray();
        var incident = array[0]!.AsObject();
        incident["id"] = id;
        incident["ended_at_ms"] = endedAtMs;
        var frozen = incident["findings"]!.AsArray();
        var first = frozen[0]!.ToJsonString();
        frozen.Clear();
        for (var index = 0; index < findings; index++)
            frozen.Add(JsonNode.Parse(first));
        return Assert.Single(IncidentParser.Parse(Encoding.UTF8.GetBytes(array.ToJsonString())).Incidents);
    }

    [Fact]
    public async Task A_pushed_incident_is_stored_like_a_polled_one()
    {
        var cancellationToken = TestContext.Current.CancellationToken;
        var id = "a1000000000000000000000000000001";

        Assert.True(await factory.Database.TryUpsertPushedIncidentsAsync(
            Source, [Incident(id, null, 2)], 1_800_000_000_000, cancellationToken));

        var stored = await factory.Database.FindIncidentAsync(id, null, cancellationToken);
        Assert.NotNull(stored);
        Assert.Equal(2, stored.FindingCount);
        Assert.Null(stored.EndedAtMs);
    }

    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public async Task Push_and_poll_converge_on_the_richer_copy_whatever_their_order(bool pushFirst)
    {
        var cancellationToken = TestContext.Current.CancellationToken;
        var id = pushFirst ? "a1000000000000000000000000000002" : "a1000000000000000000000000000003";
        var rich = Incident(id, null, 3);
        var endedButPoorer = Incident(id, 1_800_000_090_000, 1);

        if (pushFirst)
        {
            await factory.Database.TryUpsertPushedIncidentsAsync(Source, [rich], 1_800_000_000_000, cancellationToken);
            await factory.Database.UpsertIncidentsAsync(Source, [endedButPoorer], 1_800_000_100_000, cancellationToken);
        }
        else
        {
            await factory.Database.UpsertIncidentsAsync(Source, [rich], 1_800_000_000_000, cancellationToken);
            await factory.Database.TryUpsertPushedIncidentsAsync(Source, [endedButPoorer], 1_800_000_100_000,
                cancellationToken);
        }

        var stored = await factory.Database.FindIncidentAsync(id, null, cancellationToken);
        Assert.NotNull(stored);
        Assert.Equal(3, stored.FindingCount);
        Assert.Equal(1_800_000_090_000, stored.EndedAtMs);
    }

    [Fact]
    public async Task A_push_records_the_import_and_files_no_read()
    {
        var cancellationToken = TestContext.Current.CancellationToken;
        await factory.Database.TryUpsertPushedIncidentsAsync(
            Source, [Incident("a1000000000000000000000000000004", null, 1)], 1_800_000_200_000, cancellationToken);

        await using var connection = await factory.Database.OpenConnectionAsync(cancellationToken);
        await using var command = connection.CreateCommand();
        command.CommandText = """
                              SELECT (SELECT last_import_ms FROM source_imports WHERE source_id = 'test'),
                                     (SELECT COUNT(*) FROM incident_reads WHERE source_id = 'test');
                              """;
        await using var reader = await command.ExecuteReaderAsync(cancellationToken);
        Assert.True(await reader.ReadAsync(cancellationToken));
        Assert.Equal(1_800_000_200_000, reader.GetInt64(0));
        Assert.Equal(0, reader.GetInt64(1));
    }
}
```

If another test class in the shared fixture files an incidents read for source `test` before this one runs, the last assertion is order-dependent: in that case read `incident_reads` before and after the push and assert the row did not change instead of asserting a count of 0.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `dotnet test PerfSentinelHub.Tests --filter-class PerfSentinelHub.Tests.IncidentImportStorageTests`
Expected: build error, `TryUpsertPushedIncidentsAsync` does not exist.

- [ ] **Step 3: Extract the `source_imports` upsert**

In `HubDatabase.cs`, replace the `if (!fromPoll) { ... }` block in `UpsertBatchCoreAsync` with a call, and add the helper:

```csharp
        if (!fromPoll)
            await RecordImportAsync(connection, transaction, source.SourceId, observedAtMs, cancellationToken);
```

```csharp
    /// <summary>When this source last pushed, findings or incidents alike.</summary>
    private static async Task RecordImportAsync(
        SqliteConnection connection,
        SqliteTransaction transaction,
        string sourceId,
        long observedAtMs,
        CancellationToken cancellationToken)
    {
        await using var imported = connection.CreateCommand();
        imported.Transaction = transaction;
        imported.CommandText = """
                               INSERT INTO source_imports(source_id, last_import_ms)
                               VALUES ($source_id, $observed_at)
                               ON CONFLICT(source_id) DO UPDATE SET last_import_ms = excluded.last_import_ms;
                               """;
        imported.Parameters.AddWithValue(SourceIdParameter, sourceId);
        imported.Parameters.AddWithValue(ObservedAtParameter, observedAtMs);
        await imported.ExecuteNonQueryAsync(cancellationToken);
    }
```

- [ ] **Step 4: Add the bounded upsert**

In `HubDatabase.Incidents.cs`, below `UpsertIncidentsAsync`:

```csharp
    /// <summary>
    ///     Stores a daemon's pushed incidents under the poll's merge rule, so a
    ///     push and a poll of one incident converge in either order. Waits for
    ///     the write lock as long as a findings push does, then gives up: a 503
    ///     the daemon retries beats a request held open behind a purge. Files
    ///     the push, not a read: incident_reads stays the poll's own ledger.
    /// </summary>
    public async Task<bool> TryUpsertPushedIncidentsAsync(
        string sourceId,
        IReadOnlyList<ParsedIncident> incidents,
        long observedAtMs,
        CancellationToken cancellationToken)
    {
        if (!await _writeGate.WaitAsync(WriteGateWait, cancellationToken))
            return false;
        try
        {
            await using var connection = await OpenConnectionAsync(cancellationToken);
            await using var transaction = connection.BeginTransaction(false);
            foreach (var incident in incidents)
                await UpsertIncidentAsync(connection, transaction, sourceId, incident, observedAtMs, cancellationToken);
            await RecordImportAsync(connection, transaction, sourceId, observedAtMs, cancellationToken);
            await transaction.CommitAsync(cancellationToken);
            return true;
        }
        finally
        {
            _writeGate.Release();
        }
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `dotnet test PerfSentinelHub.Tests --filter-class PerfSentinelHub.Tests.IncidentImportStorageTests` then `dotnet test PerfSentinelHub.Tests --filter-class PerfSentinelHub.Tests.ImportApiTests`
Expected: PASS for both (the second proves the extraction kept the findings push intact).

- [ ] **Step 6: Commit**

```bash
git add PerfSentinelHub/Storage/HubDatabase.cs PerfSentinelHub/Storage/HubDatabase.Incidents.cs PerfSentinelHub.Tests/IncidentImportStorageTests.cs
git commit -S -m "feat(import): store pushed incidents under the poll's merge rule"
```

---

### Task 3: Count refused incident imports

**Files:**
- Modify: `PerfSentinelHub/Api/ImportMetrics.cs`
- Modify: `PerfSentinelHub/Program.cs:75-78`
- Modify: `PerfSentinelHub/Api/MetricsEndpoint.cs` (`MapMetrics`, `RenderAsync`, after the `perf_sentinel_hub_import_rejected_total` block)
- Test: `PerfSentinelHub.Tests/MetricsEndpointTests.cs`

**Interfaces:**
- Produces: `public sealed class IncidentImportMetrics : ImportMetrics` and `public sealed record IncidentImportAdmission(ImportGate Gate, IncidentImportMetrics Metrics)` with `public T Refuse<T>(ImportRejection reason, T result)`, both registered as singletons.

- [ ] **Step 1: Write the failing test** (append to `MetricsEndpointTests`, next to the test at line ~228 that checks the findings import series)

```csharp
    [Fact]
    public async Task Refused_incident_imports_are_published_by_reason_from_startup()
    {
        var body = await _client.GetStringAsync("/metrics", TestContext.Current.CancellationToken);

        Assert.Contains("# TYPE perf_sentinel_hub_incident_import_rejected_total counter", body,
            StringComparison.Ordinal);
        foreach (var reason in new[] { "bad_request", "unauthorized", "gate_full", "write_timeout", "too_large" })
            Assert.Matches(
                $"perf_sentinel_hub_incident_import_rejected_total\\{{reason=\"{reason}\"\\}} \\d+", body);
    }
```

Use the field or helper the surrounding tests use to get the client (`_client` or a local `Client()`), matching the test at line ~228.

- [ ] **Step 2: Run it to verify it fails**

Run: `dotnet test PerfSentinelHub.Tests --filter-method "*Refused_incident_imports_are_published_by_reason_from_startup"`
Expected: FAIL, the series is absent.

- [ ] **Step 3: Implement**

In `ImportMetrics.cs`, unseal the counter and add the incident pair:

```csharp
public class ImportMetrics
```

```csharp
/// <summary>
///     The incident import's refusals, on a series of their own: a label added to
///     the findings counter would change what an unaggregated alert on it matches.
/// </summary>
public sealed class IncidentImportMetrics : ImportMetrics;

/// <summary>The incident import's admission: the shared gate, its own refusal count.</summary>
public sealed record IncidentImportAdmission(ImportGate Gate, IncidentImportMetrics Metrics)
{
    public T Refuse<T>(ImportRejection reason, T result)
    {
        Metrics.Rejected(reason);
        return result;
    }
}
```

In `Program.cs`, after `builder.Services.AddSingleton<ImportAdmission>();`:

```csharp
builder.Services.AddSingleton<IncidentImportMetrics>();
builder.Services.AddSingleton<IncidentImportAdmission>();
```

In `MetricsEndpoint.cs`, add `IncidentImportMetrics incidentImports` to the `MapGet` lambda parameters and to `RenderAsync` (after `ImportMetrics imports`, passed through), and after the `perf_sentinel_hub_import_rejected_total` loop:

```csharp
        Counter(text, "perf_sentinel_hub_incident_import_rejected_total",
            "Incident imports the Hub refused, by reason, published from startup like the findings ones.");
        foreach (var (reason, count) in incidentImports.Snapshot())
            Line(text, "perf_sentinel_hub_incident_import_rejected_total", $"reason=\"{reason}\"", count);
```

Update every other caller of `RenderAsync` the compiler names.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `dotnet test PerfSentinelHub.Tests --filter-class PerfSentinelHub.Tests.MetricsEndpointTests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add PerfSentinelHub/Api/ImportMetrics.cs PerfSentinelHub/Program.cs PerfSentinelHub/Api/MetricsEndpoint.cs PerfSentinelHub.Tests/MetricsEndpointTests.cs
git commit -S -m "feat(metrics): count refused incident imports by reason"
```

---

### Task 4: The incident import route

**Files:**
- Modify: `PerfSentinelHub/Api/ApiEndpoints.cs` (route map at line ~75, `ImportFindingsAsync` at ~143, `ReadBodyAsync` callers, logger messages at ~463)
- Test: `PerfSentinelHub.Tests/IncidentImportApiTests.cs` (create)

**Interfaces:**
- Consumes: `IncidentParser.ParseImport` (Task 1), `HubDatabase.TryUpsertPushedIncidentsAsync` (Task 2), `IncidentImportAdmission` (Task 3), the existing `ImportResponse`, `IsAuthorized`, `HasValidUtf8`, `ReadBodyAsync(request, ct, maxBodyBytes)`.
- Produces: `POST /api/import/incidents?source_id=<id>`.

- [ ] **Step 1: Write the failing tests**

Create `PerfSentinelHub.Tests/IncidentImportApiTests.cs`:

```csharp
using System.Globalization;
using System.Net;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.Extensions.DependencyInjection;
using PerfSentinelHub.Api;

namespace PerfSentinelHub.Tests;

public sealed class IncidentImportApiTests(HubApplicationFactory factory) : IClassFixture<HubApplicationFactory>
{
    private const string ApiKey = "0123456789abcdef0123456789abcdef"; // gitleaks:allow -- synthetic test credential
    private readonly HttpClient _client = factory.CreateClient();

    private static JsonObject FixtureIncident(string id)
    {
        var array = JsonNode.Parse(File.ReadAllText(Path.Combine(
            AppContext.BaseDirectory, "Fixtures", "daemon-incidents-0.20.0.json")))!.AsArray();
        var incident = array[0]!.AsObject();
        incident["id"] = id;
        return incident;
    }

    private static HttpRequestMessage Request(string? apiKey, IEnumerable<JsonNode> incidents,
        string query = "source_id=test")
    {
        var body = new JsonObject
        {
            ["producer_version"] = "0.29.0",
            ["incidents"] = new JsonArray(incidents.Select(node => node.DeepClone()).ToArray())
        };
        var request = new HttpRequestMessage(HttpMethod.Post, "/api/import/incidents?" + query)
        {
            Content = new StringContent(body.ToJsonString(), Encoding.UTF8, "application/json")
        };
        if (apiKey is not null)
            request.Headers.Add("X-API-Key", apiKey);
        return request;
    }

    private static string Id(int index)
    {
        return "b" + index.ToString("x31", CultureInfo.InvariantCulture);
    }

    [Theory]
    [InlineData(null)]
    [InlineData("wrong-key-wrong-key-wrong-key-wrong")]
    public async Task The_route_answers_to_the_source_import_key_alone(string? apiKey)
    {
        using var request = Request(apiKey, [FixtureIncident(Id(1))]);
        using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

        Assert.Equal(HttpStatusCode.Unauthorized, response.StatusCode);
    }

    [Fact]
    public async Task An_unknown_source_is_refused_as_a_wrong_key_is()
    {
        using var request = Request(ApiKey, [FixtureIncident(Id(2))], "source_id=nobody");
        using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

        Assert.Equal(HttpStatusCode.Unauthorized, response.StatusCode);
    }

    [Fact]
    public async Task A_pushed_incident_is_stored_and_acknowledged_after_the_commit()
    {
        var cancellationToken = TestContext.Current.CancellationToken;
        using var request = Request(ApiKey, [FixtureIncident(Id(3))]);
        using var response = await _client.SendAsync(request, cancellationToken);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var body = JsonDocument.Parse(await response.Content.ReadAsByteArrayAsync(cancellationToken));
        Assert.Equal(1, body.RootElement.GetProperty("accepted").GetInt32());
        Assert.Equal(0, body.RootElement.GetProperty("rejected").GetInt32());
        Assert.NotNull(await factory.Database.FindIncidentAsync(Id(3), null, cancellationToken));
    }

    [Fact]
    public async Task An_invalid_incident_is_refused_alone()
    {
        var cancellationToken = TestContext.Current.CancellationToken;
        using var request = Request(ApiKey, [FixtureIncident(Id(4)), FixtureIncident("NOT-A-HEX-ID")]);
        using var response = await _client.SendAsync(request, cancellationToken);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var body = JsonDocument.Parse(await response.Content.ReadAsByteArrayAsync(cancellationToken));
        Assert.Equal(1, body.RootElement.GetProperty("accepted").GetInt32());
        Assert.Equal(1, body.RootElement.GetProperty("rejected").GetInt32());
    }

    [Fact]
    public async Task A_batch_of_only_invalid_incidents_is_a_bad_request()
    {
        using var request = Request(ApiKey, [FixtureIncident("NOT-A-HEX-ID")]);
        using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    [Fact]
    public async Task More_than_ten_incidents_are_too_large()
    {
        using var request = Request(ApiKey, Enumerable.Range(10, 11).Select(index => FixtureIncident(Id(index))));
        using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

        Assert.Equal(HttpStatusCode.RequestEntityTooLarge, response.StatusCode);
    }

    [Fact]
    public async Task A_body_over_four_mebibytes_is_too_large()
    {
        var incident = FixtureIncident(Id(30));
        incident["detail"] = new string('x', 4 * 1024 * 1024);
        using var request = Request(ApiKey, [incident]);
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
            using var request = Request(ApiKey, [FixtureIncident(Id(31))]);
            using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

            Assert.Equal(HttpStatusCode.ServiceUnavailable, response.StatusCode);
            Assert.Equal("1", response.Headers.GetValues("Retry-After").Single());
        }
        finally
        {
            for (var slot = 0; slot < entered; slot++)
                gate.Exit();
        }
    }

    [Fact]
    public async Task A_second_query_parameter_is_a_bad_request()
    {
        using var request = Request(ApiKey, [FixtureIncident(Id(32))], "source_id=test&extra=1");
        using var response = await _client.SendAsync(request, TestContext.Current.CancellationToken);

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `dotnet test PerfSentinelHub.Tests --filter-class PerfSentinelHub.Tests.IncidentImportApiTests`
Expected: FAIL with 404 Not Found on every request.

- [ ] **Step 3: Share the admission prelude**

In `ApiEndpoints.cs`, add the helper and rewrite the head of `ImportFindingsAsync` to use it, so the two handlers share one copy of the checks:

```csharp
    /// <summary>
    ///     What every import checks before reading its body: the query names one
    ///     source, the key is that source's, and a gate slot is free. Null when
    ///     the request may proceed, the refusal otherwise, already counted. The
    ///     caller owns the slot and exits it.
    /// </summary>
    private static IResult? AdmitImport(
        HttpRequest request,
        IOptions<HubOptions> options,
        ImportGate gate,
        Func<ImportRejection, IResult, IResult> refuse,
        out SourceOptions source)
    {
        source = null!;
        if (!HasValidUtf8(request.QueryString.Value) ||
            request.Query.Count != 1 ||
            !request.Query.TryGetValue("source_id", out var sourceIds) ||
            sourceIds.Count != 1 ||
            string.IsNullOrEmpty(sourceIds[0]))
            return refuse(ImportRejection.BadRequest, TypedResults.BadRequest());
        var match = options.Value.Sources.FirstOrDefault(candidate =>
            string.Equals(candidate.Id, sourceIds[0], StringComparison.Ordinal));
        if (match is null || !IsAuthorized(request, match.ImportApiKey))
            return refuse(ImportRejection.Unauthorized, TypedResults.Unauthorized());
        if (!gate.TryEnter())
        {
            request.HttpContext.Response.Headers.RetryAfter = "1";
            return refuse(ImportRejection.GateFull,
                TypedResults.StatusCode(StatusCodes.Status503ServiceUnavailable));
        }

        source = match;
        return null;
    }
```

`ImportFindingsAsync` now starts with:

```csharp
        if (AdmitImport(request, options, admission.Gate, admission.Refuse<IResult>, out var source) is { } refused)
            return refused;
```

and its `try { ... } finally { admission.Gate.Exit(); }` body is unchanged.

- [ ] **Step 4: Add the incident handler and route**

```csharp
        app.MapPost("/api/import/incidents", ImportIncidentsAsync).AllowAnonymous();
```

next to the findings import mapping, and the handler below `ImportFindingsAsync`:

```csharp
    // One incident carries up to a thousand frozen findings: the cap of one daemon page.
    private const int IncidentImportMaxBytes = 4 * 1024 * 1024;

    private static async Task<IResult> ImportIncidentsAsync(
        HttpRequest request,
        HubDatabase database,
        IncidentImportAdmission admission,
        IOptions<HubOptions> options,
        TimeProvider timeProvider,
        ILoggerFactory loggerFactory,
        CancellationToken cancellationToken)
    {
        if (AdmitImport(request, options, admission.Gate, admission.Refuse<IResult>, out var source) is { } refused)
            return refused;
        try
        {
            var payload = await ReadBodyAsync(request, cancellationToken, IncidentImportMaxBytes);
            if (payload is null)
                return admission.Refuse(ImportRejection.TooLarge,
                    TypedResults.StatusCode(StatusCodes.Status413PayloadTooLarge));

            ParsedIncidentImport import;
            try
            {
                import = IncidentParser.ParseImport(payload);
            }
            catch (ImportBatchTooLargeException)
            {
                return admission.Refuse(ImportRejection.TooLarge,
                    TypedResults.StatusCode(StatusCodes.Status413PayloadTooLarge));
            }
            catch (InvalidDataException)
            {
                return admission.Refuse(ImportRejection.BadRequest, TypedResults.BadRequest());
            }

            if (import.Page.Incidents.Count == 0)
                return admission.Refuse(ImportRejection.BadRequest, TypedResults.BadRequest());
            if (!await database.TryUpsertPushedIncidentsAsync(source.Id, import.Page.Incidents,
                    timeProvider.GetUtcNow().ToUnixTimeMilliseconds(), cancellationToken))
            {
                request.HttpContext.Response.Headers.RetryAfter = "1";
                return admission.Refuse(ImportRejection.WriteTimeout,
                    TypedResults.StatusCode(StatusCodes.Status503ServiceUnavailable));
            }

            if (import.Page.RejectedCount > 0)
                LogRejectedImportedIncidents(loggerFactory.CreateLogger("ImportApi"), source.Id,
                    import.Page.RejectedCount);
            return TypedResults.Ok(new ImportResponse(import.Page.Incidents.Count, import.Page.RejectedCount));
        }
        finally
        {
            admission.Gate.Exit();
        }
    }
```

and beside event 1301:

```csharp
    [LoggerMessage(1302, LogLevel.Warning, "Source {SourceId} rejected {RejectedCount} imported incidents.")]
    private static partial void LogRejectedImportedIncidents(
        ILogger logger,
        string sourceId,
        int rejectedCount);
```

If Sonar or `dotnet build` flags the two handlers' parse-and-refuse tails as duplicated, keep them: they differ in parser, store and log, and folding them behind delegates costs more reading than it saves. Check the cognitive complexity of both stays at most 15.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `dotnet test PerfSentinelHub.Tests --filter-class PerfSentinelHub.Tests.IncidentImportApiTests` then the whole suite: `dotnet test PerfSentinelHub.Tests`
Expected: PASS everywhere, `ImportApiTests`, `AccessRoutesTests` and `AuthenticationTests` included.

- [ ] **Step 6: Commit**

```bash
git add PerfSentinelHub/Api/ApiEndpoints.cs PerfSentinelHub.Tests/IncidentImportApiTests.cs
git commit -S -m "feat(import): accept daemon incidents pushed to /api/import/incidents"
```

---

### Task 5: Document the route and close the documented gap

**Files:**
- Modify: `docs/API.md` (`## Import API`) and `docs/FR/API-FR.md` (`## API d'import`)
- Modify: `docs/OPERATIONS.md` (`## Freshness and recovery`, the incidents paragraph, and the metrics table near line 76) and `docs/FR/OPERATIONS-FR.md`
- Modify: `docs/ARCHITECTURE.md` (`## Push and poll, and which one owns reachability`) and `docs/FR/ARCHITECTURE-FR.md`
- Modify: `CHANGELOG.md` (`## [Unreleased]`)

- [ ] **Step 1: API.md**, after the paragraph on `POST /api/import/findings`, add:

```markdown
`POST /api/import/incidents?source_id=<id>` accepts a daemon's incidents, pushed by perf-sentinel
0.29.0 and later beside its findings: `{"producer_version":"…","incidents":[…]}`, each incident in
the shape `GET /api/incidents` serves on the daemon. It takes the same key, the same four-slot gate
and the same five-second write wait, at most 10 incidents and 4 MiB per request, since one incident
carries up to a thousand frozen findings. A pushed incident is merged as a polled one is: the copy
with more frozen findings wins and an end is kept once seen, so push and poll converge in any
order. Refusals count on `perf_sentinel_hub_incident_import_rejected_total`.
```

French mirror in `docs/FR/API-FR.md`, same place:

```markdown
`POST /api/import/incidents?source_id=<id>` accepte les incidents d'un daemon, poussés par
perf-sentinel 0.29.0 et suivants à côté de ses findings : `{"producer_version":"…","incidents":[…]}`,
chaque incident sous la forme que sert `GET /api/incidents` sur le daemon. Il prend la même clé, la
même porte à quatre places et la même attente d'écriture de cinq secondes, au plus 10 incidents et
4 Mio par requête, puisqu'un incident porte jusqu'à mille findings gelés. Un incident poussé est
fusionné comme un incident lu : la copie qui porte le plus de findings gelés l'emporte et une fin
est gardée une fois vue, donc push et poll convergent dans n'importe quel ordre. Les refus se
comptent sur `perf_sentinel_hub_incident_import_rejected_total`.
```

- [ ] **Step 2: OPERATIONS.md.** In `## Freshness and recovery`, replace the sentence ending "can be lost between two polls, which the daemon's own NDJSON archive covers." with:

```markdown
Incidents are read on the same hourly poll as the findings. A daemon on perf-sentinel 0.29.0 or
later with `[daemon.hub_export]` on also pushes them as they are recorded, closed or completed, so
an incident its ring evicts between two polls still arrives. Without that push, one captured
minutes before a daemon restart can be lost between two polls, which the daemon's own NDJSON
archive covers.
```

Add a row to the metrics table beside `perf_sentinel_hub_import_rejected_total`:

```markdown
| `perf_sentinel_hub_incident_import_rejected_total{reason}` | counter | Incident imports refused, by the same reasons as the findings import |
```

French mirror, same places, with:

```markdown
Les incidents sont lus au même poll horaire que les findings. Un daemon en perf-sentinel 0.29.0 ou
suivant avec `[daemon.hub_export]` actif les pousse aussi quand ils sont enregistrés, fermés ou
complétés, donc un incident que son anneau évince entre deux polls arrive quand même. Sans ce push,
un incident capturé quelques minutes avant le redémarrage d'un daemon peut se perdre entre deux
polls, ce que couvre l'archive NDJSON du daemon.
```

```markdown
| `perf_sentinel_hub_incident_import_rejected_total{reason}` | counter | Imports d'incidents refusés, pour les mêmes raisons que l'import des findings |
```

Match the table's existing column widths.

- [ ] **Step 3: ARCHITECTURE.md.** In the push and poll section, add after its first paragraph:

```markdown
Incidents follow the same split from perf-sentinel 0.29.0: the daemon pushes each one when it is
recorded, closed or completed, and the hourly poll still reads them as a safety net. Both write
through one merge rule, so neither path can undo the other.
```

French mirror:

```markdown
Les incidents suivent le même partage depuis perf-sentinel 0.29.0 : le daemon pousse chacun quand
il est enregistré, fermé ou complété, et le poll horaire les lit toujours comme filet de sécurité.
Les deux écrivent par une même règle de fusion, donc aucun chemin ne peut défaire l'autre.
```

Leave `docs/diagrams/svg/push-and-poll*.svg` untouched: the diagrams are drawn by hand. Note for the owner, in the PR description: the diagram should show incidents on the push arrow as well as findings.

- [ ] **Step 4: CHANGELOG.md**, under `## [Unreleased]` (create the heading and an `### Added` section if missing):

```markdown
### Added

- `POST /api/import/incidents?source_id=<id>` takes the incidents a daemon on perf-sentinel 0.29.0
  or later pushes beside its findings, with the same key and gate, at most 10 incidents and 4 MiB
  per request. An incident its ring evicts between two hourly polls now reaches the Hub. The poll
  still reads incidents, and both paths merge under one rule. Refusals count on
  `perf_sentinel_hub_incident_import_rejected_total{reason}`.
```

- [ ] **Step 5: Verify and commit**

Run: `make verify-fast` (pinned SDK on PATH)
Expected: exit 0, format, tests, coverage and doc checks green.

```bash
git add docs/API.md docs/FR/API-FR.md docs/OPERATIONS.md docs/FR/OPERATIONS-FR.md docs/ARCHITECTURE.md docs/FR/ARCHITECTURE-FR.md CHANGELOG.md
git commit -S -m "docs: describe the incident import and the gap it closes"
```
