# CLI reference

This page documents the user-facing subcommands of the `perf-sentinel`
binary. For deeper architecture and design notes, see
[`ARCHITECTURE.md`](./ARCHITECTURE.md). For runtime hooks (CI gates,
exit codes, env vars), see [`CI.md`](./CI.md) and
[`RUNBOOK.md`](./RUNBOOK.md).

A full inventory of options is also available via `--help` on each
subcommand:

```bash
perf-sentinel --help
perf-sentinel <subcommand> --help
```

The sections below are not exhaustive for every subcommand. They
focus on the user surfaces that benefit from prose context (workflow,
defaults, exit codes). For exhaustive flag listings, prefer `--help`.

## capture

Receives OTLP traces and writes them to a file, so a CI job can produce
the input `analyze --ci` gates on without running an OpenTelemetry
Collector. It only receives and writes, it never analyzes: the verdict
stays with `analyze`, on a file you can keep as a build artifact, replay
with different thresholds, or compare against a baseline with `diff`.

The application needs no perf-sentinel-specific setting, only the
standard endpoint variables. Set the protocol too: SDKs disagree on
their default, and an endpoint pointed at the wrong one exports nothing
without saying so.

```bash
# OTLP HTTP, what most SDKs default to
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf

# OTLP gRPC, the opentelemetry-java default
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
export OTEL_EXPORTER_OTLP_PROTOCOL=grpc
```

### Two shapes

**Wrapping the test step**, which is the sturdier one:

```bash
perf-sentinel capture --output traces.json -- mvn verify
perf-sentinel analyze --ci --input traces.json
```

The ports are bound before the command starts, so no export is lost to a
start-up race, and the capture ends when the command does rather than on
a guessed delay. The command inherits stdout and stderr untouched, and
its exit code is propagated: a failing test run stays a failing job.

The wrapped command is whatever runs your tests. `capture` starts a
process, it does not care what that process is, and nothing about it is
Java-specific:

```bash
perf-sentinel capture --output traces.json -- ./gradlew integrationTest
perf-sentinel capture --output traces.json -- pytest tests/integration
perf-sentinel capture --output traces.json -- npm run test:e2e
perf-sentinel capture --output traces.json -- go test ./...
perf-sentinel capture --output traces.json -- dotnet test
perf-sentinel capture --output traces.json -- ./scripts/run-integration-tests.sh
```

The command is executed directly, not through a shell, so a pipeline or
a `&&` chain belongs in a script that `capture` then wraps.

What does differ per language is the instrumentation that makes the
application export in the first place, covered in
[`INSTRUMENTATION.md`](./INSTRUMENTATION.md). `capture` only listens.

**Tests running in containers.** When the application under test runs in
a sibling container rather than on the runner itself, bind the listener
on all interfaces and point the exporter at the host:

```bash
perf-sentinel capture --output traces.json --listen-address 0.0.0.0 -- docker compose up --exit-code-from tests
# in the container: OTEL_EXPORTER_OTLP_ENDPOINT=http://host.docker.internal:4318
#                   OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
```

**Alongside an existing test step**, when your pipeline owns the test
command and it cannot be prefixed:

```bash
perf-sentinel capture --output traces.json &
CAPTURE=$!
./scripts/run-integration-tests.sh
kill -TERM $CAPTURE && wait $CAPTURE
```

> **Prefix the existing step, never add a second one.** `capture -- mvn
> verify` runs the tests once. A new pipeline stage next to the existing
> one would run the whole integration suite twice, for nothing.

### Output

NDJSON, one OTLP request per line, the shape the Collector `file`
exporter produces. `analyze`, `report` and `diff` auto-detect it, no
flag needed. Requests are written as received, unconverted, so the file
describes what the application sent.

The directory of `--output` is created if it is missing, so
`--output target/traces.json` works on a clean CI workspace where the
build tool has not created `target/` yet. In wrapper mode the build tool
has not run yet.

Progress and the final count go to **stderr**, never stdout: in wrapper
mode that stream belongs to the wrapped command. The summary is how you
tell "no anti-patterns found" from "nothing was ever exported", and an
empty trace file is rejected by `analyze` rather than reported as a
clean gate.

### Options worth knowing

| Flag                                        | Default         | Why you would change it                                                                |
|---------------------------------------------|-----------------|----------------------------------------------------------------------------------------|
| `--listen-address`                          | `127.0.0.1`     | `0.0.0.0` when the application runs in another container of the same job               |
| `--listen-port-grpc` / `--listen-port-http` | `4317` / `4318` | a port is already taken on the agent                                                   |
| `--max-file-size`                           | `512` (MiB)     | a large suite. Past the cap the file stays valid but incomplete, and the run exits `2` |
| `--grace-ms`                                | `2000`          | how long to keep listening after the command exits, for the exporter's last flush      |

### Exit codes

- `0`: capture completed.
- the wrapped command's own code when it failed, since that is the more
  important signal. A command killed by a signal reports `128 + signal`,
  as a shell would, never `0`.
- `1`: the capture itself failed (port taken, unwritable file). Both are
  detected before the wrapped command starts, so a capture that cannot
  listen never leaves a test suite running.
- `2`: the trace file is short of the run, either because the size cap
  was hit or because requests could not be queued fast enough. Any
  verdict from it would understate the run.

## ack

Acknowledge findings via the daemon ack API introduced in 0.5.20.
Three subactions: `create`, `revoke`, `list`.

The CLI consumes the daemon's HTTP endpoints
(`POST/DELETE /api/findings/{sig}/ack` and `GET /api/acks`). It does
not edit the TOML CI baseline
(`.perf-sentinel-acknowledgments.toml`). That file is meant to be
edited by hand and shipped via PR review. See
[`ACK-WORKFLOW.md`](./ACK-WORKFLOW.md) for guidance on choosing
between the two ack mechanisms.

### Synopsis

```bash
perf-sentinel ack [OPTIONS] <SUBCOMMAND>
```

Top-level options (apply to all subactions):

- `--daemon <URL>`: daemon HTTP endpoint. Defaults to
  `$PERF_SENTINEL_DAEMON_URL` then `http://localhost:4318`.

### `ack create`

Create a new acknowledgment.

```bash
perf-sentinel ack create \
  --signature "n_plus_one_sql:order-svc:_api_orders:0123456789abcdef0123456789abcdef" \
  --reason "deferred to next sprint" \
  --expires 7d
```

Options:

- `--signature <SIG>` (or `-s`): finding signature to acknowledge. If
  omitted, the CLI reads it from stdin (only when stdin is not a TTY).
  The stdin read is capped at 1 KiB so a `cat /dev/urandom` pipe cannot
  exhaust memory before the daemon-side validator rejects the input.
- `--reason <TEXT>` (or `-r`): required, free-form description of why
  the finding is being acked.
- `--expires <ISO8601_OR_DURATION>`: ack expiration. Accepts ISO8601
  datetimes (`2026-05-11T00:00:00Z`) or relative durations (`7d`,
  `24h`, `30m`). Omit for a permanent ack.
- `--by <NAME>`: identity of the acker. Falls back to `$USER`, then
  `"anonymous"`.
- `--api-key-file <PATH>`: see "Authentication" below.

### `ack revoke`

Remove an existing acknowledgment.

```bash
perf-sentinel ack revoke \
  --signature "n_plus_one_sql:order-svc:_api_orders:0123456789abcdef0123456789abcdef"
```

### `ack list`

Enumerate active daemon acknowledgments.

```bash
perf-sentinel ack list
perf-sentinel ack list --output json
```

`ack list` only shows daemon-side acks. TOML CI acks remain visible
in `.perf-sentinel-acknowledgments.toml` itself. The daemon caps the
response at 1000 entries. The `AT` and `EXPIRES_AT` columns are in
local time, `--output json` stays in UTC.

### Authentication

When the daemon enforces an API key (`[daemon.ack] api_key` in the
daemon config), the CLI resolves it in priority order:

1. `PERF_SENTINEL_DAEMON_API_KEY` environment variable.
2. `--api-key-file <PATH>`. The file's content is read and any
   trailing newline is stripped.
3. Interactive `rpassword` prompt (no echo) if the daemon returns 401
   and stdin is a TTY. The pasted value is capped at 1 KiB.

`query` takes the same `--api-key-file <PATH>` once, before or after
its subcommand, and resolves it the same way (steps 1 and 2, no prompt).
Every `query` subcommand sends the key on each request, so a daemon with
`[daemon] require_read_key = true`, which refuses every read without a
key, answers `findings`, `explain`, `correlations` and `status` too. A
refused read exits 1 and names the 401. The read key `[daemon]
read_api_key` suffices for every `query` read. `ack`, and acknowledging
from `query inspect`, need the ack key. Every other reader of such a
daemon needs the key too: PerfSentinelHub, Grafana datasources, `curl
…/api/export/report | perf-sentinel report`, the live HTML report and
the JetBrains plugin. Do not turn the switch on for a daemon developers'
IDEs read until the JetBrains plugin installed there sends a
per-endpoint credential: earlier releases send none and get `401` on
every refresh. See [QUERY-API.md](QUERY-API.md#read-gate).

```bash
perf-sentinel query --api-key-file ~/.config/perf-sentinel/read.key findings
perf-sentinel query explain --trace-id abc123 --api-key-file ~/.config/perf-sentinel/read.key
```

There is no `--api-key <SECRET>` flag because passing secrets on the
command line leaks them via the process list and shell history.

On Unix, `--api-key-file` is opened with `O_NOFOLLOW` (symlinks are
refused) and the CLI prints a one-line warning on stderr if the file
is group/world readable (`mode & 0o077 != 0`). The warning is gated
behind a TTY check on stderr: in CI / Docker / systemd contexts where
stderr is not a TTY, the warning is suppressed to keep build logs
clean. Operators running in those environments should set the file
mode declaratively (k8s Secret with `defaultMode: 0o400`, a
`StatefulSet` mounted from a `Secret`, etc.) rather than relying on
the runtime warning.

### Daemon URL resolution

`--daemon <URL>` > `PERF_SENTINEL_DAEMON_URL` env > default
`http://localhost:4318`. The default matches `perf-sentinel watch`,
which listens on the OTLP/HTTP standard port.

### Exit codes

- `0`: success.
- `1`: generic error (network failure, parse error, missing
  signature on stdin).
- `2`: client error (HTTP 4xx). Includes 401 (unauthorized), 409
  (already acknowledged), 404 (not acknowledged on revoke), 400
  (invalid signature format).
- `3`: server error (HTTP 5xx). Includes 503 (ack store disabled),
  500 (write failure) and 507 (ack store full).

Errors are written to stderr with a one-line cause and an actionable
hint when applicable.

## Hub: login, logout, hub

`login`, `logout` and `hub` talk to a
[Perf Sentinel Hub](https://github.com/Institut-du-Numerique-Responsable/PerfSentinelHub),
not to a daemon. Each `hub` action's help line starts with "Through the
Hub:", which tells `hub analyze`, `hub ack` and `hub findings` apart from
`analyze`, `ack` and `query findings`.

### `login` and `logout`

```bash
perf-sentinel login --hub https://hub.example.com
# Open https://hub.example.com/device in a browser and enter the code BCDF-GHJK (expires in 10 minutes).
# Signed in to https://hub.example.com until 2026-10-14 18:42:00 (local time).
```

`login` runs the Hub's device flow; `--hub` falls back to
`PERF_SENTINEL_HUB_URL`. It prints the Hub's own `/device`
address and a code, and opens nothing: you type the code on that page, in
a browser signed in to the Hub, and approve. Only approve a code from a
`login` you started yourself, just now. The CLI polls at the interval
the Hub gives, slower by 5 seconds each time the Hub answers
`slow_down`, until the code is approved, denied or expires (exit 1 on the
last two). The token lasts as long as the browser session that approved
it, and there is no refresh: run `login` again when it expires.

A Hub without terminal sign-in, because its access control is off,
`TrustedProxyNetworks` is unset or it predates the feature, answers the
first request with a redirect, a `401` or a `404`: `login` then says
"This Hub does not offer terminal sign-in" and exits 1.

The token goes to `$XDG_CONFIG_HOME/perf-sentinel/hub-token.json`, else
`~/.config/perf-sentinel/hub-token.json`, or
`%APPDATA%\perf-sentinel\hub-token.json` on Windows, with the Hub URL and
the expiry. One Hub at a time: a `login` to another Hub replaces it. On
Unix the directory is created `0700` and the file `0600`, written beside
the target and renamed over it. A token file that is a symlink, that other
users can read (`mode & 0o077 != 0`, as ssh refuses a private key), or that
has expired is refused before any request; for a file other users can
read, run `logout`, which still revokes its token.

`logout` revokes the token on the Hub (`POST /auth/revoke`) and deletes
the file, whatever the Hub answers. When the Hub is unreachable it warns
that the token stays valid until its expiry. A file other users can read
is revoked too, with a warning. A symlink or a file that does not parse
is deleted without being revoked, since it yields no token to send, and
`logout` exits 1: a token in it stays valid on the Hub until it expires.

### `hub`

```bash
perf-sentinel hub findings --environment production --status active --severity critical
perf-sentinel hub incidents --source-id prod-eu
perf-sentinel hub incidents 0123456789abcdef0123456789abcdef
perf-sentinel hub ack --source-id prod-eu -s "<signature>" -r "deferred to next sprint" --expires 7d
perf-sentinel hub unack --source-id prod-eu -s "<signature>"
perf-sentinel hub analyze --source-id tempo --service order-svc --lookback 1h -o report.html
```

| Action | Calls | Flags |
|---|---|---|
| `findings` | `GET /api/findings` | `--service`, `--finding-type`, `--severity`, `--status`, `--source-id`, `--environment`, `--from`/`--to` (RFC 3339 UTC such as `2026-10-21T00:00:00Z`, or epoch milliseconds; a `--from` later than `--to` is refused before any request), `--offset`, `--include-acked true\|false` (the Hub default when omitted), `--signature`, `--limit` (50), `--format text\|json` |
| `incidents [ID]` | `GET /api/incidents[/{id}]` | `--service`, `--namespace`, `--source-id`, `--environment`, `--offset`, `--limit` (50), `--format` |
| `ack` | `POST /api/sources/{source_id}/acks` | `--source-id`, `-s/--signature` (stdin when omitted), `-r/--reason`, `--expires`. No `--by`: the Hub names you |
| `unack` | `POST /api/sources/{source_id}/acks/revoke` | `--source-id`, `-s/--signature` |
| `analyze` | `POST /api/analyses`, `GET /api/analyses/{id}` every 2 s, then `GET /reports/{id}.html` | `--source-id`, `--service`, `--lookback`, `--from`/`--to` (RFC 3339 UTC or epoch milliseconds, sent as `from_ms`/`to_ms`), `--trace-id`, `--max-traces`, `-o/--output` (default `hub-report-<id>.html`) |

Text output uses the `query findings` and `query incidents` layouts. A
listing of incidents carries no findings, so each says how many it froze
and `hub incidents <ID>` shows them. `--format json` prints the Hub's
answer unchanged. `analyze` exits 1 with the run's `error_code` when the
run fails, is interrupted or expires.

The credential, in this order:

1. `PERF_SENTINEL_HUB_TOKEN`, a terminal or machine token. It needs a Hub
   URL from `--hub` or `PERF_SENTINEL_HUB_URL`.
2. The token `login` stored, whose Hub is then the default. A `--hub`
   naming another Hub is refused ("The stored token was issued by ...").
3. Neither: the request goes out with no `Authorization` header and the
   Hub decides. A Hub without access control answers `GET /api/findings`
   anonymously.

A token only travels over `https`, or plain `http` to `localhost`,
`127.0.0.1` or `::1`; any other `http` URL is refused before a request.
The token never appears on the command line, in a log line or in an error
message. No redirect is followed: a `3xx` means the Hub wants a browser
session, either because the route needs `perf-sentinel login` or because
the Hub's access control is off and that route needs a session. A `401`
prints the `login` command to run, a `403`, `404` or `409` the Hub's own
`detail`, and a `503` without one "Hub busy, retry". Every error exits 1.

## Other subcommands

`perf-sentinel query incidents [--service <NAME>] [--namespace <NAME>]
[--offset N] [--limit N] [--format text|json] [--api-key-file <PATH>]`
lists the incidents the alerting posted to the daemon (0.20.0+,
`[daemon.incidents]`), newest first. Each incident gets one header block
(kind, service as `ns/service` when the alert carried a namespace, start
and end as local time, the window, a capture marker that says whether
the ring still held the whole window, how many findings fired only after
the restart, the alert's detail and the id), then its findings in the
`query findings` layout. `--service` and `--namespace` narrow the list
to one value each. `--limit` defaults to 50 and the daemon caps it
at 100. `--offset` pages past the newest. A 401 (pass the key), a 503
(`[daemon.incidents] enabled = false`) and a 404 (daemon older than
0.20.0) each exit 1 with their cause named. `query monitor
--api-key-file <PATH>` hands the same key to every monitor poll, the
Incidents tab included, see [`INSPECT.md`](./INSPECT.md).

For now, see `perf-sentinel <subcommand> --help` for the exhaustive
option lists of `analyze`, `watch`, `query`, `report`, `diff`,
`explain`, `inspect`, `pg-stat`, `mysql-stat`, `tempo`, `jaeger-query`, `demo`,
`bench` and `calibrate`. The commands themselves are stable. Their
prose documentation is being filled in incrementally.

The supply-chain commands have dedicated prose documentation elsewhere:
`disclose`, `disclose-pdf`, `verify-hash` and `hash-bake` are covered in
[`REPORTING.md`](./REPORTING.md), with the signing and provenance
background in [`SUPPLY-CHAIN.md`](./SUPPLY-CHAIN.md). The share card
commands, `card keygen`, `card render` and `verify-card`, are covered in
[`SHARE-CARD.md`](./SHARE-CARD.md), with their JSON lines, exit codes
and payloads. Shell completions and the man page are documented below
on this page.

## Times on the terminal

Every time printed for a person is in the local time zone: the
`Window:` line of `analyze`, `report` and `diff` text output, the
`inspect` panels, and the `ack`, `query`, `login`, `logout` and `hub`
output. JSON, SARIF,
NDJSON, CSV and the daemon API stay in UTC.

The zone comes from `TZ` when it is set, then from the system. A `TZ`
that names a zone the system database lacks resolves against an IANA
database embedded in the binary instead of falling back to UTC. That
is the case in the `FROM scratch` container image, which ships no
time zone database, so `docker run -e TZ=Europe/Paris ...` prints
Paris time. A POSIX rule such as `CET-1CEST,M3.5.0,M10.5.0/3` works
too.

## Shell completions

`perf-sentinel completions <shell>` writes a completion script to
stdout. Supported shells: `bash`, `zsh`, `fish`, `powershell`,
`elvish`. Pipe the output to the shell-specific completion path:

```bash
# Zsh (oh-my-zsh, prezto, manual fpath)
perf-sentinel completions zsh > ~/.zfunc/_perf-sentinel

# Bash
perf-sentinel completions bash > /usr/local/etc/bash_completion.d/perf-sentinel

# Fish
perf-sentinel completions fish > ~/.config/fish/completions/perf-sentinel.fish
```

Reload your shell, or `source` the file, after install. Re-run the
generator after upgrading `perf-sentinel` so completions stay in sync
with new flags and subcommands.

## Man page

`perf-sentinel man` writes a roff man page to stdout. It renders the
top-level page, which lists the subcommands (like `git.1`). Redirect it
into your man path:

```bash
perf-sentinel man > /usr/local/share/man/man1/perf-sentinel.1
```

`man perf-sentinel` then works. To preview without installing:

```bash
perf-sentinel man > /tmp/perf-sentinel.1 && man /tmp/perf-sentinel.1
```

Re-run the generator after upgrading `perf-sentinel` so the page stays
in sync with new flags and subcommands.
