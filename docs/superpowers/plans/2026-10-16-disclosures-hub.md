# Disclosures in the Hub, Hub and lab side, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** PerfSentinelHub 0.7.0 receives disclosures a CI publishes, checks and renders them with its embedded engine (0.31.0), keeps them, and hands them on through a restricted Disclosures screen, expiring share links that serve files only, and signed webhooks.

**Architecture:** A new `Disclosures/` area holds the renderer call, the share-link download counter and the webhook delivery worker. Storage is schema V12 (three tables, files in BLOBs). Routes split into the anonymous import, the permission-gated API, and the anonymous `/share/` file route. The launcher gains a Disclosures screen built from pure helpers in `launcher.js`.

**Tech Stack:** .NET 10 (SDK 10.0.401 in `~/.dotnet`), ASP.NET Core minimal APIs, NativeAOT analyzers on, Microsoft.Data.Sqlite, xUnit 3 on MTP, node `--test` for `launcher.js`, Playwright for screenshots.

**Spec:** `docs/superpowers/specs/2026-10-16-disclosures-in-hub-design.md` (in the perf-sentinel repository)

**Repository:** `/Users/robintrassard/RiderProjects/PerfSentinelHub`, branch `feat/disclosures` cut from `main` once 0.6.0 (PerfSentinelHub#95) is merged and released. Environment: `export DOTNET_ROOT=$HOME/.dotnet PATH=$HOME/.dotnet:$PATH`. One test class at a time with `-class`.

## Global Constraints

- Disclosures are on when `Hub:Disclosures:ImportKey` is set. Without it: the import route answers 404, `/share/` answers 404, the tab is hidden, and every existing response is byte for byte unchanged except the permission list (`AccessOffCompatibilityTests` guard this).
- Permissions: `disclosures.read`, `disclosures.manage`, in `AccessPermissions.All` only. Not in reader or member, not in `MachineTokenPermissions`. Refused on a project grant, like `admin`.
- Import: `POST /api/import/disclosures`, anonymous, `X-API-Key` compared with SHA-256 and `FixedTimeEquals` (`IsAuthorized`), `multipart/form-data`, parts `report` (required), `attestation`, `bundle`. 8 MiB per request, checked on `Content-Length` and while reading. Its own gate of one (503 + `Retry-After: 1`). 60 seconds for the whole request.
- Engine contract (from the engine plan): `disclose-pdf --report --attestation --bundle --expected-identity --expected-issuer --output`, exit 0 rendered, 1 refused, 3 input error, 5 cannot render, one JSON line `{"result":...,"reason":...}` on stdout. The Hub reads the exit code and that line only, never stderr. Reasons as shipped in 0.31.0: exit 1 `content_hash_mismatch`, `binding_mismatch`. Exit 3 `missing_file`, `malformed_report`, `output_unwritable`. Exit 5 `unsupported_schema_version`, `unsupported_characters`, `pdf_export_failed`. `--expected-identity` and `--expected-issuer` go together or not at all (clap exits 2 otherwise, with no JSON line), so the options validation requires both or neither.
- Import outcomes: 0 stores with the PDF. 1 answers 422 with `content_hash_mismatch` or `binding_mismatch`, nothing stored. 3 answers 422 `malformed_report`, nothing stored. 5, or an engine without `disclose-pdf`, stores without a PDF and the reason. An internal report with `AllowInternal` off answers 422 `internal_refused` before any engine run.
- Idempotence: same `report_uuid`, same bytes: 200, nothing changes. Same uuid and `content_hash`, stored copy without attestation, new copy with one: the new copy replaces it, PDF rendered again, `signature_added_at` set, no webhook. Anything else under the same uuid: 409.
- "Internal" is `report_metadata.confidentiality_level == "internal"`. Internal reports are never shared nor pushed.
- Files are stored exactly as received, never re-serialised.
- Share links: token `pshare_` + base64url of 32 random bytes, shown once, stored as SHA-256. Default `ShareLinkDays` 7, maximum 30. Public reports only, not withdrawn. File names served: `report.json`, `attestation.intoto.jsonl`, `bundle.sig`, `report.pdf`, `SHA256SUMS`, nothing else. One identical 404 for unknown, expired, revoked, withdrawn. Headers `Referrer-Policy: no-referrer`, `Cache-Control: no-store`, `X-Content-Type-Options: nosniff`, `X-Robots-Tag: noindex`. No SQLite write per request, counts flushed once a minute.
- Webhooks: config only (`Name`, `Url`, `Secret`, `Confidentiality`, `IncludeShareLink`, `ShareLinkDays`), https (http to loopback only), secret 32+ characters, `Confidentiality` must be `public`. Events `published` and `withdrawn`. Headers `X-PerfSentinel-Timestamp` (Unix seconds) and `X-PerfSentinel-Signature: sha256=<lowercase hex HMAC-SHA256 of "<timestamp>.<body>">`. URL never stored nor shown.
- Startup validation: `ImportKey` 32+ characters, no control characters, distinct from every source `ImportApiKey` and machine-token secret. `AllowInternal` requires access control on.
- Metrics: `perf_sentinel_hub_disclosure_import_rejected_total{reason}` on the import reasons' closed set plus `render_refused`, `malformed_report`, `internal_refused`, and `perf_sentinel_hub_disclosure_deliveries_failed_total`.
- Prose rules for docs and UI copy: no em dash, no semicolon, straight quotes, sentence case, FR mirrors with accents in the same commit.
- `make verify-fast` green at every task end (format, python and js tests, coverage gate, analysis config). Commits signed, subject only.

## Review Focus

1. A CI that publishes the unsigned report, then the signed one, must end with the signed files and one "published" webhook. Pinned in Task 4.
2. A share link must keep answering 404 the moment its disclosure is withdrawn, even if its counter has unflushed downloads. Pinned in Task 7.
3. A Hub restarted with deliveries pending must resume them without sending any twice once acknowledged. Pinned in Task 8.
4. An engine older than 0.31.0 (no `disclose-pdf`) must store the files and say "not checked here", never 500. Pinned in Task 4.
5. A multipart body with a duplicate `report` part or an unknown part must be refused, not merged. Pinned in Task 4.

## File Structure

- `PerfSentinelHub/Configuration/HubOptions.cs` (`DisclosuresOptions`, `WebhookOptions`, permissions), `HubOptionsValidator.Disclosures.cs` (new partial).
- `PerfSentinelHub/Storage/Schema.cs` (V12), `Storage/HubDatabase.Disclosures.cs` (new).
- `PerfSentinelHub/Disclosures/DisclosureEnvelope.cs` (parses the fields the Hub needs from `report.json`), `DisclosureRenderer.cs` (the engine call), `ShareDownloadCounter.cs` (in-memory counts + flusher), `WebhookDelivery.cs` (signing, body), `DeliveryWorker.cs` (hosted service).
- `PerfSentinelHub/Analysis/EngineProbe.cs` (`SupportsDisclosePdf`).
- `PerfSentinelHub/Api/ApiEndpoints.Disclosures.cs` (import, read, actions), `ApiEndpoints.Share.cs` (`/share/`), `ImportMetrics.cs`, `MetricsEndpoint.cs`.
- `PerfSentinelHub/wwwroot/launcher.js` (pure helpers), `app.js` (screen), `index.html` (tab), `launcher.css`.
- Tests in `PerfSentinelHub.Tests/` and `tests/launcher.test.js`. `FakeEngine.cs` gains a `disclose-pdf` mode.

---

### Task 1: Options, validation and permissions

**Files:** `HubOptions.cs`, `HubOptionsValidator.Disclosures.cs`, `HubOptionsValidator.Access.cs` (project-grant refusal), `Api/AccessPolicy.cs` if `All` is not picked up automatically. Test: `ConfigurationTests.cs`, `AccessRoutesTests.cs`, `AccessOffCompatibilityTests.cs`.

**Interfaces:**
- Produces: `DisclosuresOptions { string? ImportKey; bool AllowInternal; string? ExpectedIdentity; string? ExpectedIssuer; int ShareLinkDays = 7; IReadOnlyList<WebhookOptions> Webhooks }`, `HubOptions.Disclosures`, `bool DisclosuresOptions.Enabled => ImportKey is not null`, `AccessPermissions.DisclosuresRead`, `AccessPermissions.DisclosuresManage`.

- [ ] **Step 1: Failing tests:** `import_key_shorter_than_32_is_refused`, `import_key_equal_to_a_source_import_key_is_refused`, `import_key_equal_to_a_machine_token_secret_is_refused`, `allow_internal_without_access_control_is_refused`, `share_link_days_above_30_is_refused`, `webhook_http_url_to_a_remote_host_is_refused` (http to `127.0.0.1` accepted), `webhook_internal_confidentiality_is_refused`, `webhook_short_secret_is_refused`, `disclosures_permissions_on_a_project_grant_are_refused`, `admin_holds_both_disclosure_permissions_and_reader_member_do_not`, `machine_tokens_never_hold_disclosure_permissions`.
- [ ] **Step 2: Run** `dotnet test --project PerfSentinelHub.Tests -- -class PerfSentinelHub.Tests.ConfigurationTests` (and the access classes). Expected: FAIL.
- [ ] **Step 3: Implement.** Bind with the existing trimming convention. Error messages follow the validator's existing tone.
- [ ] **Step 4: Run again,** expected PASS, then `make verify-fast`.
- [ ] **Step 5: Commit** `feat(config): add the disclosures options and permissions`

### Task 2: Schema V12 and storage

**Files:** `Storage/Schema.cs`, `Storage/HubDatabase.cs` (append V12, record migration 12), `Storage/HubDatabase.Disclosures.cs`. Test: `DisclosureStorageTests.cs`.

**Interfaces:**
- Produces tables `disclosures`, `disclosure_share_links`, `disclosure_deliveries` with the spec's columns, plus `signature_added_at_ms`, `render_reason` (nullable) on `disclosures`. Methods (all `async`, `CancellationToken` last):
  `InsertDisclosureAsync(DisclosureRecord)`, `ReplaceSignedCopyAsync(string id, DisclosureFiles files, byte[]? pdf, long nowMs)`, `FindDisclosureByUuidAsync(string reportUuid)`, `GetDisclosureAsync(string id) -> DisclosureSummary?` (every column but the BLOBs, plus which files exist), `ReadFileAsync(string id, DisclosureFile file)` (one BLOB), `ListDisclosuresAsync() -> IReadOnlyList<DisclosureSummary>`, `WithdrawAsync(string id, string reason, string by, long nowMs)`, `SetPdfAsync(string id, byte[]? pdf, string? reason, string engineVersion)`, `InsertShareLinkAsync`, `FindShareLinkAsync(byte[] tokenHash, long nowMs)` (joins the disclosure, returns null when expired, revoked or withdrawn), `RevokeShareLinkAsync`, `AddShareDownloadsAsync(IReadOnlyDictionary<string,long>)`, `EnqueueDeliveriesAsync`, `DueDeliveriesAsync(long nowMs)`, `RecordDeliveryAttemptAsync`.
- `DisclosureFile` enum: `Report`, `Attestation`, `Bundle`, `Pdf`.

- [ ] **Step 1: Failing tests:** `a_disclosure_round_trips_its_files_byte_for_byte`, `report_uuid_is_unique`, `a_withdrawn_disclosure_kills_its_share_links`, `an_expired_or_revoked_link_is_not_found`, `the_list_never_loads_blobs` (assert via a 5 MiB report that `ListDisclosuresAsync` stays under a few KB of allocation or simply that `DisclosureSummary` has no byte fields), `download_counts_add_up`, `due_deliveries_come_back_after_a_reopen`, `backup_carries_the_files` (run `HubBackup.RunAsync`, open the copy, read the PDF BLOB).
- [ ] **Step 2: Run** `-class PerfSentinelHub.Tests.DisclosureStorageTests`, expected FAIL.
- [ ] **Step 3: Implement.** Index `disclosures(period_to DESC, id)`. Files read with `GetStream` or `GetBytes` per file, never in the list query.
- [ ] **Step 4: Run, expected PASS,** then `make verify-fast`.
- [ ] **Step 5: Commit** `feat(storage): keep published disclosures and their links in schema V12`

### Task 3: Engine probe and renderer

**Files:** `Analysis/EngineProbe.cs`, `Disclosures/DisclosureEnvelope.cs`, `Disclosures/DisclosureRenderer.cs`, `PerfSentinelHub.Tests/FakeEngine.cs`. Test: `EngineProbeTests.cs`, `DisclosureRendererTests.cs`.

**Interfaces:**
- Produces: `EngineProbe.SupportsDisclosePdf` (a third concurrent probe, `disclose-pdf --help` exiting 0, inside the existing 5 s budget). `DisclosureEnvelope.TryParse(ReadOnlySpan<byte> report, out DisclosureEnvelope envelope)` reading `schema_version`, `report_metadata.{report_uuid, intent, confidentiality_level, integrity_level, generated_at}`, `organisation.name`, `period.{type, from, to}`, `integrity.content_hash`, with `JsonDocument` and depth and size limits. `DisclosureRenderer.RenderAsync(DisclosureFiles files, CancellationToken) -> RenderOutcome` where `RenderOutcome` is `Rendered(byte[] Pdf)`, `Refused(string Reason)`, `Malformed(string Reason)`, `Unrenderable(string Reason)`, `Unsupported` (no `disclose-pdf`). Runs `EngineProcess.RunAsync` in a private temporary directory deleted in `finally`, with `--expected-identity` and `--expected-issuer` from options, 4 KiB stdout cap, the PDF read back with a 32 MiB cap.
- `FakeEngine` gains `DisclosePdfExitCode`, `DisclosePdfOutput` (the JSON line), and writes a fixed `%PDF-1.7` file to `--output` on exit 0. Its `--help` probe answers per subcommand.

- [ ] **Step 1: Failing tests:** probe detects and misses `disclose-pdf`. Renderer maps each exit code to its outcome. An unknown reason on stdout maps to `Malformed("engine_contract")`. A missing or non-JSON stdout line likewise. A timeout maps to `Unrenderable("timeout")`. The temporary directory is gone after each outcome. The envelope parser refuses a missing uuid and reads an internal report.
- [ ] **Step 2: Run** the two classes, expected FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expected PASS,** then `make verify-fast`.
- [ ] **Step 5: Commit** `feat(disclosures): render a received disclosure with the embedded engine`

### Task 4: `POST /api/import/disclosures`

**Files:** `Api/ApiEndpoints.Disclosures.cs`, `Api/ApiEndpoints.cs` (route map, event id for the log line), `Api/ImportMetrics.cs`, `Api/MetricsEndpoint.cs`, `Program.cs` (gate singleton). Test: `DisclosureImportApiTests.cs`, `MetricsEndpointTests.cs`.

**Interfaces:**
- Consumes: Tasks 1 to 3.
- Produces: response `{"id": "...", "pdf": "rendered" | "not_checked", "reason": null | "...", "content_hash_checked": bool, "binding_checked": bool | null}`. Status 201 on a new disclosure, 200 on an idempotent repeat or a signed copy replacing an unsigned one, 401, 404 (disclosures off), 409, 413, 415 (not multipart), 422 with `{"reason": "..."}`, 503.
- Enqueues one `published` delivery per accepting webhook on 201 only.

- [ ] **Step 1: Failing tests,** each with `FakeEngine`:
  `disabled_hub_answers_404`, `wrong_key_is_401_and_counted`, `rendered_publication_is_201_with_the_pdf_stored`, `edited_figures_are_422_content_hash_mismatch_and_nothing_stored`, `foreign_attestation_is_422_binding_mismatch`, `garbage_report_is_422_malformed_report`, `exit_5_stores_without_pdf_with_the_reason`, `engine_without_disclose_pdf_stores_as_not_checked`, `internal_report_with_allow_internal_off_is_422_before_any_engine_run` (assert the fake engine never ran), `same_bytes_twice_is_200_unchanged`, `signed_copy_replaces_the_unsigned_one_without_a_second_webhook`, `same_uuid_other_hash_is_409`, `body_over_8_mib_is_413`, `duplicate_report_part_is_422_malformed_report`, `unknown_part_is_422_malformed_report`, `second_concurrent_publication_is_503`, `metrics_count_each_rejection_reason`.
- [ ] **Step 2: Run** `-class PerfSentinelHub.Tests.DisclosureImportApiTests`, expected FAIL.
- [ ] **Step 3: Implement** with `request.ReadFormAsync` under `FormOptions` limits set from the 8 MiB cap (AOT-safe, no model binding), each part read through a capped stream.
- [ ] **Step 4: Run, expected PASS,** then `make verify-fast`.
- [ ] **Step 5: Commit** `feat(api): accept disclosures published by a CI`

### Task 5: Reading and actions

**Files:** `Api/ApiEndpoints.Disclosures.cs`. Test: `DisclosuresApiTests.cs`, `AccessRoutesTests.cs`.

**Interfaces:**
- Produces: `GET /api/disclosures`, `GET /api/disclosures/{id}` (summary, checks, `signature_added_at`, share links without tokens, deliveries by webhook name), `GET /api/disclosures/{id}/files/{name}` (the five fixed names, `SHA256SUMS` computed on the fly in `sha256sum` format, two spaces, file name), behind `disclosures.read`. `POST /api/disclosures/{id}/share-links` (body `{"days": n}`, returns the token once), `DELETE /api/disclosures/{id}/share-links/{linkId}`, `POST /api/disclosures/{id}/withdraw` (body `{"reason": "..."}`, 1 to 500 characters), `POST /api/disclosures/{id}/deliveries/{deliveryId}/retry`, `POST /api/disclosures/{id}/render`, behind `disclosures.manage`. Writes need the existing antiforgery or same-origin rule the other write routes use. `/api/status` gains `"disclosures": true` only when disclosures are on, so an off Hub's status stays byte for byte the same.

- [ ] **Step 1: Failing tests:** read routes 403 without `disclosures.read`, 404 when off. List newest period first with withdrawn marked. Files byte for byte with the right `Content-Type`. `SHA256SUMS` verifies against the stored bytes. Share link refused on an internal report (422 `internal_not_shareable`) and on a withdrawn one (409). Days above `ShareLinkDays` maximum refused. Withdraw enqueues `withdrawn` deliveries only for disclosures that had `published` ones. Render-again turns `not_checked` into `rendered` once the fake engine supports `disclose-pdf`. Every manage route 403 for a reader with `disclosures.read` only.
- [ ] **Step 2: Run** the classes, expected FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expected PASS,** then `make verify-fast`.
- [ ] **Step 5: Commit** `feat(api): read, share and withdraw disclosures`

### Task 6: The Disclosures screen

**Files:** `wwwroot/launcher.js`, `app.js`, `index.html`, `launcher.css`. Test: `tests/launcher.test.js`, Playwright demo specs and fixtures (`tests/browser/demo/`), screenshots.

**Interfaces:**
- Produces in `launcher.js`: `periodRail(disclosures)` (quarters of the span, each `published`, `withdrawn` or `missing`), `coverageNotch(coverage)` (`{percent, below}` against 0.75), `checkRows(detail)` (the "who checked what" rows, `declared` or `checked`, never `checked` for the signature), `verifyCommand(detail, expected)`, `downloadAs(url, filename)` (fetch, 401 to sign-in, Blob, `URL.createObjectURL`, revoke after 4 s). `PSL.screenOf` gains `disclosures`.
- Tab shown when `/api/status` says disclosures are on and the caller holds `disclosures.read` (or access control is off).

- [ ] **Step 1: Failing node tests** for each helper, including `checkRows` never marking a signature `checked`, and `downloadAs` sending a 401 to the sign-in route.
- [ ] **Step 2: Run** `node --test tests/launcher.test.js`, expected FAIL.
- [ ] **Step 3: Implement the helpers, then the screen** with the impeccable skill (`/impeccable` craft floor), from the 14 October mock-ups (artifact "Divulgations dans le Hub": `list-light/dark`, `detail-light/dark`, `list-mobile-light`), declared facts dashed, Hub checks solid, 390 px without horizontal scroll, both themes.
- [ ] **Step 4: Run** node tests, then `npx playwright test` on the demo config with new fixtures, expected PASS. Regenerate the Disclosures screenshots only.
- [ ] **Step 5: Commit** `feat(launcher): add the Disclosures screen`

### Task 7: `/share/{token}/{name}`

**Files:** `Api/ApiEndpoints.Share.cs`, `Disclosures/ShareDownloadCounter.cs`, `Program.cs`, `deploy/helm/perf-sentinel-hub/templates/configmap.yaml` (hosting log level Warning). Test: `DisclosureShareTests.cs`.

**Interfaces:**
- Produces: `ShareDownloadCounter.Add(string linkId)`, `FlushAsync()`, a hosted service flushing every 60 s and on shutdown.

- [ ] **Step 1: Failing tests:** each of the five files served with the four headers and `Content-Disposition: attachment. Filename=<name>`. `/share/{token}/` and any other name 404. Unknown, expired, revoked and withdrawn tokens give the same 404 body and headers. No SQLite write during a burst of 100 downloads (count rows' `last_used_at` unchanged until `FlushAsync`). Withdrawn after downloads were counted still 404 at once. `verify-hash --url` path shape: `report.json`, `attestation.intoto.jsonl` and `bundle.sig` all under one prefix.
- [ ] **Step 2: Run** `-class PerfSentinelHub.Tests.DisclosureShareTests`, expected FAIL.
- [ ] **Step 3: Implement.** The route is anonymous, does not go through the access policy, and never renders HTML. Client address recorded only with `TrustedProxyNetworks`.
- [ ] **Step 4: Run, expected PASS,** `make helm-lint helm-template`, then `make verify-fast`.
- [ ] **Step 5: Commit** `feat(api): serve shared disclosure files under an expiring token`

### Task 8: Webhooks

**Files:** `Disclosures/WebhookDelivery.cs`, `Disclosures/DeliveryWorker.cs`, `Program.cs` (named `HttpClient` "disclosure-webhooks", `AllowAutoRedirect = false`, `UseCookies = false`, `Hub:HttpTimeout`). Test: `DisclosureWebhookTests.cs`.

**Interfaces:**
- Produces: `WebhookDelivery.Sign(string secret, long timestamp, ReadOnlySpan<byte> body) -> string` (`sha256=` + lowercase hex), `WebhookDelivery.Body(DisclosureSummary, string event, string? shareUrl) -> byte[]` with `event`, `disclosure_id`, `organisation`, `period`, `content_hash`, `intent`, `integrity_level`, `share_url`.
- Worker: polls due deliveries, sends, records the attempt, backs off (curve and cap are the builder's), creates the share link per delivery when `IncludeShareLink` is set.

- [ ] **Step 1: Failing tests:** signature equals a fixed vector (secret `"s" * 32`, timestamp `1800000000`, body `{}`). A receiver behind a local `HttpListener` gets `published` with valid headers. A 500 is retried later and a 2xx stops retries. A 302 is a failure, never followed. An internal disclosure enqueues nothing. A restart with a due row sends it once. `perf_sentinel_hub_disclosure_deliveries_failed_total` counts final failures. The detail API shows webhook names, never URLs.
- [ ] **Step 2: Run** `-class PerfSentinelHub.Tests.DisclosureWebhookTests`, expected FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expected PASS,** then `make verify-fast`.
- [ ] **Step 5: Commit** `feat(disclosures): push signed webhooks on publication and withdrawal`

### Task 9: Docs

**Files:** `docs/API.md`, `CONFIGURATION.md`, `ACCESS-CONTROL.md`, `OPERATIONS.md` (two metric families, backup size, log level), `LAUNCHER.md`, `ARCHITECTURE.md`, the `SECURITY-OVERVIEW` route inventory (two anonymous routes), `CHANGELOG.md` `[Unreleased]`, all FR mirrors. `OPERATIONS.md` family count goes from eleven to thirteen.

- [ ] **Step 1: Write them.**
- [ ] **Step 2:** `make verify-fast`.
- [ ] **Step 3: Commit** `docs: describe publishing, sharing and pushing disclosures`

### Task 10: Lab scenario `hub-disclosures`

**Repository:** `~/IdeaProjects/perf-sentinel-simulation-lab`, commits on `main`, never the local manifest pins.

**Files:** `scenarios/hub-disclosures/{verify.sh,README.md}`, `Makefile` target, `docs/SCENARIOS.md`.

- [ ] **Step 1:** Build local images (`seed-daemon-local` with the engine branch, `seed-hub-local` with this branch). The Hub reached through TLS with a lab CA passed to `verify-hash` in `SSL_CERT_FILE`.
- [ ] **Step 2: Write `verify.sh`** with the spec's five steps: publish (201, PDF rendered), share link and `verify-hash --url` (PARTIAL, content hash and binding Ok), `verify-hash --report` on the downloaded PDF (same), edited figures (422 `content_hash_mismatch`, nothing stored), internal report (422), webhook receiver checking the HMAC of `published` then `withdrawn`, and the link answering 404 after the withdrawal.
- [ ] **Step 3: Run it,** expected PASS.
- [ ] **Step 4: Commit** `feat(hub-disclosures): publish, share, verify and withdraw a disclosure`

### Task 11: Release 0.7.0

Only once engine 0.31.0 is published.

- [ ] **Step 1:** Repin the engine to 0.31.0 by digest in `Dockerfile` and `config/supply-chain.json` (`docker buildx imagetools inspect`), bump the project, `Dockerfile`, chart `version` and `appVersion`, READMEs, `CONTRIBUTING.md`, `RELEASING.md` examples, CHANGELOG `[0.7.0] - <date>`.
- [ ] **Step 2:** `make release-check VERSION=0.7.0`, `make verify`, the lab pass on the final images, one ledger line.
- [ ] **Step 3: Commit** `chore(release): 0.7.0`
