/**
 * perf-sentinel Hub launcher — pure logic.
 *
 * Classic script, no module syntax, no build step: the page loads it with a plain
 * <script src> and reads it off `window.PSL`. Brief §3.5 forbids a build stage in
 * the shipped app, so this file is authored as runnable JavaScript and type-checked
 * separately with `tsc --noEmit` against `types.d.ts`, which `npm run typecheck`
 * at the repository root runs.
 *
 * Everything here is a pure function or a frozen table. Rendering, state and DOM
 * live in the page.
 */
(function (global) {
    'use strict';

    /**
     * The analysis binary embedded in the Hub, and the Hub service itself. Both
     * come from `/api/status` at load time rather than being baked in: a value
     * frozen here would go stale the first time either side is upgraded, and
     * `skew()` would then compare against a version nobody is running.
     */
    let ENGINE = null;
    let HUB = null;

    /**
     * @param {string | null} hub
     * @param {string | null} engine
     * @returns {void}
     */
    function setVersions(hub, engine) {
        HUB = hub;
        ENGINE = engine;
    }

    /**
     * A code-to-sentence table read from the text tables on each access, so it follows the
     * shown language without being rebuilt.
     * @param {string} prefix
     * @param {string[]} codes
     * @returns {Record<string, string>}
     */
    function textTable(prefix, codes) {
        /** @type {Record<string, string>} */
        const table = {};
        codes.forEach(function (code) {
            Object.defineProperty(table, code, {enumerable: true, get: () => t(prefix + code)});
        });
        return table;
    }

    /**
     * One actionable sentence per code, naming the next action. The service refuses to
     * expose raw stderr, so this table is the entire failure vocabulary the operator gets.
     * @type {Record<import('../types').ErrorCode, string>}
     */
    const ERRORS = textTable('error.run.', ['source_unreachable', 'source_auth_failed', 'source_rejected_request',
        'timeout', 'output_too_large', 'binary_failed', 'invalid_request', 'hub_memory_low', 'internal']);

    /**
     * What a failed read of a source means. A different vocabulary from ERRORS,
     * which describes a failed analysis run: these are the codes the collector
     * records, from a settings read or from a poll, the last three from a poll only.
     */
    const READ_ERRORS = textTable('error.read.', ['network_error', 'http_error', 'timeout', 'invalid_status',
        'response_too_large', 'hub_busy', 'invalid_findings', 'database_full', 'storage_error']);

    /**
     * The health a source's row shows. A full database is the Hub's, so it is named
     * as such rather than as the daemon being down, and a source minting new
     * signatures is flagged while it still answers.
     * @param {import('../types').Source} source
     * @param {number} now
     * @returns {{tone: 'ok' | 'warn' | 'crit', text: string}}
     */
    /**
     * What a failed read of one run means. Only a 404 says the Hub holds no such run:
     * a 503 while the Hub restarts, a gateway error or a dropped connection says
     * nothing about the run, so the page keeps what it knows and asks again.
     * @param {{status?: number}} error
     * @returns {'unknown' | 'retry'}
     */
    function runReadOutcome(error) {
        return error && error.status === 404 ? 'unknown' : 'retry';
    }

    /**
     * How long to wait before asking again after `failures` reads in a row failed:
     * one second, doubled each time, at most fifteen.
     * @param {number} failures
     * @returns {number}
     */
    function retryDelay(failures) {
        return Math.min(1000 * Math.pow(2, failures - 1), 15000);
    }

    function sourceHealth(source, now) {
        if (source.last_error_code === 'database_full' && !source.reachable)
            return {tone: 'crit', text: t('health.database_full')};
        if (!source.reachable)
            return {tone: 'crit', text: t('health.unreachable', {duration: dur(now - source.unreachable_since_ms)})};
        if (source.signature_rate_warning)
            return {tone: 'warn', text: t('health.signature_rate', {count: source.new_signatures_last_hour})};
        return {tone: 'ok', text: source.last_attempt_ms == null ? t('health.unobserved') : t('health.reachable')};
    }

    /** @type {Record<import('../types').ErrorCode, string>} */
    const ERROR_TITLES = textTable('error.title.', ['source_unreachable', 'source_auth_failed',
        'source_rejected_request', 'timeout', 'output_too_large', 'binary_failed', 'invalid_request', 'hub_memory_low',
        'internal']);

    /** @type {Record<import('../types').SourceKind, string>} */
    const KIND_LABEL = {daemon: 'daemon', tempo: 'tempo', jaeger_query: 'victoria traces'};

    const UNIT_MS = {m: 60000, h: 3600000, d: 86400000};

    /**
     * Coarse duration. Drops a zero remainder so an exact hour reads "1 h", not "1 h 0 m".
     * @param {number | null | undefined} ms
     * @returns {string}
     */
    function dur(ms) {
        if (ms == null) return t('unit.na');
        const s = Math.max(0, Math.round(ms / 1000));
        if (s < 60) return s + ' ' + t('unit.s');
        const m = Math.floor(s / 60);
        if (m < 60) return s % 60 ? m + ' ' + t('unit.min') + ' ' + (s % 60) + ' ' + t('unit.s') : m + ' ' + t('unit.min');
        const h = Math.floor(m / 60);
        if (h < 24) return m % 60 ? h + ' ' + t('unit.h') + ' ' + (m % 60) + ' ' + t('unit.min') : h + ' ' + t('unit.h');
        const d = Math.floor(h / 24);
        return h % 24 ? d + ' ' + t('unit.d') + ' ' + (h % 24) + ' ' + t('unit.h') : d + ' ' + t('unit.d');
    }

    /**
     * Every unit from the largest that applies down to `floor`, keeping the ones
     * in between even when they are zero. dur() stops at two units, which reads
     * well for an age skimmed in a table and badly for a figure someone watches:
     * "10 d" hides a whole day of drift, and a countdown never appears to move.
     * @param {number | null | undefined} ms
     * @param {'s' | 'm'} floor
     * @returns {string}
     */
    function durDown(ms, floor) {
        if (ms == null) return t('unit.na');
        const s = Math.max(0, Math.round(ms / 1000));
        const d = Math.floor(s / 86400);
        const h = Math.floor(s / 3600) % 24;
        const m = Math.floor(s / 60) % 60;
        const parts = [];
        if (d) parts.push(d + ' ' + t('unit.d'));
        if (d || h) parts.push(h + ' ' + t('unit.h'));
        if (d || h || m || floor === 'm') parts.push(m + ' ' + t('unit.min'));
        if (floor === 's') parts.push((s % 60) + ' ' + t('unit.s'));
        return parts.join(' ');
    }

    /**
     * Splits a fleet into the two kinds the launcher groups by, keeping each
     * source's position in the original array so a caller can still address a row
     * by its global index. `split` is false for a single-kind fleet, where a group
     * label would name the only thing on screen.
     * @param {{kind?: string}[]} sources
     * @returns {{daemons: {source: any, index: number}[],
     *            backends: {source: any, index: number}[], split: boolean}}
     */
    function splitByKind(sources) {
        const daemons = [];
        const backends = [];
        (sources || []).forEach(function (source, index) {
            (source && source.kind === 'daemon' ? daemons : backends).push({source: source, index: index});
        });
        return {daemons, backends, split: daemons.length > 0 && backends.length > 0};
    }

    /** A countdown, watched: down to the second. */
    function durPrecise(ms) {
        return durDown(ms, 's');
    }

    /** An uptime, re-read on an interval: down to the minute, seconds would lie. */
    function durMinutes(ms) {
        return durDown(ms, 'm');
    }

    /**
     * Same duration, split into figure and unit so a caller can set them at different
     * sizes. A 40px "m" beside a 40px digit reads as two glyphs, not one duration.
     * @param {number | null | undefined} ms
     * @returns {{n: string, u: string}[]}
     */
    function durParts(ms) {
        const s = dur(ms);
        if (ms == null) return [{n: s, u: ''}];
        /** @type {{n: string, u: string}[]} */
        const out = [];
        const re = /(\d+)\s*([a-z]+)/g;
        let m;
        while ((m = re.exec(s)) !== null) out.push({n: m[1] ?? '', u: m[2] ?? ''});
        return out;
    }

    /**
     * Local wall clock with milliseconds, for the event log.
     * @param {number} ms
     * @returns {string}
     */
    function clock(ms) {
        const d = new Date(ms);
        const p = (/** @type {number} */ n, /** @type {number} */ w) => String(n).padStart(w, '0');
        return p(d.getHours(), 2) + ':' + p(d.getMinutes(), 2) + ':' + p(d.getSeconds(), 2) + '.' + p(d.getMilliseconds(), 3);
    }

    /**
     * Relative window string, e.g. `15m`, `6h`, `90d`. Unparseable input falls back to
     * one hour rather than throwing: this feeds a live preview, not a submission.
     * @param {string} s
     * @returns {number}
     */
    function parseDur(s) {
        const m = /^(\d+)([mhd])$/.exec(s || '');
        return m ? Number(m[1]) * UNIT_MS[/** @type {'m'|'h'|'d'} */ (m[2])] : 3600000;
    }

    /**
     * @param {string} s
     * @returns {string}
     */
    function humanDur(s) {
        const m = /^(\d+)([mhd])$/.exec(s || '');
        if (!m) return s;
        const n = Number(m[1]);
        return t('unit.word.' + m[2] + (n > 1 ? '.many' : '.one'), {count: n});
    }

    /**
     * Value for an `<input type="datetime-local">`, in local time as that control expects.
     * @param {number} ms
     * @returns {string}
     */
    function dtLocal(ms) {
        const d = new Date(ms);
        const p = (/** @type {number} */ n) => String(n).padStart(2, '0');
        return d.getFullYear() + '-' + p(d.getMonth() + 1) + '-' + p(d.getDate()) + 'T' + p(d.getHours()) + ':' + p(d.getMinutes());
    }

    /**
     * @param {number} ms
     * @returns {string}
     */
    function dtHuman(ms) {
        const d = new Date(ms);
        const p = (/** @type {number} */ n) => String(n).padStart(2, '0');
        return d.getFullYear() + '-' + p(d.getMonth() + 1) + '-' + p(d.getDate()) + ' ' + p(d.getHours()) + ':' + p(d.getMinutes());
    }

    /**
     * @param {string | null | undefined} v
     * @returns {number[]}
     */
    function vparts(v) {
        return String(v || '').split('.').map(n => Number(n) || 0);
    }

    /**
     * Ordering over `major.minor.patch`. Extra segments are ignored: the Hub's own
     * version has four, and only the first three ever carry meaning here.
     * @param {string | null | undefined} a
     * @param {string | null | undefined} b
     * @returns {-1 | 0 | 1}
     */
    function vcmp(a, b) {
        const A = vparts(a), B = vparts(b);
        for (let i = 0; i < 3; i++) {
            const x = A[i] ?? 0, y = B[i] ?? 0;
            if (x !== y) return x < y ? -1 : 1;
        }
        return 0;
    }

    /**
     * The first segment where two versions differ, and by how much.
     *
     * `vcmp` decides the direction on all three segments, so the label has to
     * name the segment it decided on. Reading the minor alone worded a patch
     * gap as "0 minor behind" and a major gap in minors.
     *
     * @param {string | null | undefined} a
     * @param {string | null | undefined} b
     * @returns {{count: number, unit: string}}
     */
    function versionGap(a, b) {
        const units = ['major', 'minor', 'patch'];
        const pa = vparts(a);
        const pb = vparts(b);
        for (let i = 0; i < units.length; i++) {
            const d = Math.abs((pb[i] ?? 0) - (pa[i] ?? 0));
            if (d !== 0) return {count: d, unit: units[i]};
        }
        // `vcmp` reads the same three segments, so a caller that already
        // ruled out equality never lands here.
        return {count: 0, unit: 'patch'};
    }

    /**
     * How a producer sits against the Hub's embedded binary.
     *
     * This compares two version strings and nothing more. It cannot know whether a
     * given minor actually changed detection, so callers must word the result as
     * "may not be comparable", never as "incompatible". perf-sentinel is pre-1.0,
     * which is what makes a single minor worth surfacing at all.
     *
     * @param {string | null | undefined} producer
     * @returns {{dir: 'behind' | 'ahead', label: string, fg: string, bg: string, bd: string} | null}
     */
    function skew(producer) {
        // With no engine version there is nothing to compare against, and a
        // comparison against null would read every producer as ahead.
        if (!producer || !ENGINE) return null;
        const c = vcmp(producer, ENGINE);
        if (c === 0) return null;
        const g = versionGap(producer, ENGINE);
        return c < 0
            ? {
                dir: 'behind',
                label: t('skew.behind', {count: g.count, unit: skewUnit(g)}),
                fg: 'var(--warn-fg)',
                bg: 'var(--warn-bg)',
                bd: 'var(--warn-bd)'
            }
            : {
                dir: 'ahead',
                label: t('skew.ahead', {count: g.count, unit: skewUnit(g)}),
                fg: 'var(--info-fg)',
                bg: 'var(--info-bg)',
                bd: 'var(--info-bd)'
            };
    }

    /**
     * The segment a gap is counted in, singular or plural with its count.
     * @param {{count: number, unit: string}} gap
     * @returns {string}
     */
    function skewUnit(gap) {
        return t('skew.unit.' + gap.unit + (gap.count > 1 ? '.many' : '.one'));
    }

    /**
     * Which version string a result of this kind actually carries.
     *
     * A daemon detects its own findings, so it carries `producer`. A trace backend
     * detects nothing: the Hub's embedded binary does, so a historical read carries
     * `engine`. Labelling both "engine" is the §7.6 confusion.
     *
     * @param {import('../types').SourceKind | import('../types').Analysis} kindOrAnalysis
     * @returns {'producer' | 'engine'}
     */
    function detector(kindOrAnalysis) {
        const k = typeof kindOrAnalysis === 'string' ? kindOrAnalysis : kindOrAnalysis.kind;
        return k === 'daemon' ? 'producer' : 'engine';
    }

    /**
     * Presentation status. `empty` is derived here and never stored: it must not
     * become a seventh value of `analysis_runs.status`.
     * @param {import('../types').Analysis} a
     * @returns {import('../types').DisplayStatus}
     */
    function statusKey(a) {
        if (a.status === 'succeeded' && a.result && a.result.empty) return 'empty';
        if (a.status === 'pending') return 'queued';
        return a.status;
    }

    /**
     * One-line restatement of what was asked for. Long values are truncated by CSS,
     * never here: the full string stays available in a `title`.
     * @param {import('../types').Analysis} a
     * @returns {string}
     */
    function argsLine(a) {
        const r = /** @type {Record<string, unknown>} */ (a.request || {});
        /** @type {string[]} */
        const parts = [];
        // The engine's own argument names, never translated: this line restates what was sent.
        ['service', 'trace_id', 'lookback', 'from_ms', 'to_ms'].forEach(function (name) {
            if (r[name]) parts.push(name + ' = ' + r[name]);
        });
        if (r['max_traces'] != null) parts.push(['max_traces', r['max_traces']].join(' = '));
        const detection = /** @type {Record<string, number|string>} */ (r['detection'] || {});
        Object.keys(detection).forEach(function (name) {
            parts.push(name + ' = ' + detection[name]);
        });
        return parts.length ? parts.join('   ·   ') : t('run.args.none');
    }

    /**
     * Report-weight advice for a requested trace count.
     *
     * Bands come from the report sink (`crates/sentinel-core/src/report/html/mod.rs`):
     * it targets `DEFAULT_SIZE_TARGET_BYTES` (5 MiB) and trims to fit, findings
     * critical-first past `FINDINGS_BUDGET_SHARE_PCT` (70 %) of the budget, then
     * embedded traces lowest-waste-first. The byte size cannot be predicted before the
     * run because it depends on span counts and SQL template lengths, so these are
     * advice and not a bound. Only `ceiling` asks the operator to confirm; only `over`
     * is refused, and that refusal is the service's.
     *
     * The 500 and 1 200 boundaries come from the sink's own 5 MiB target and do
     * not move. The refusal boundary does: it is the service's configured cap,
     * and hardcoding 2 000 would refuse runs a differently configured Hub accepts.
     *
     * @param {number} n
     * @param {number} [cap]
     * @returns {{key: import('../types').WeightBand, label: string, fg: string, bg: string, bd: string, body: string, needsAck: boolean}}
     */
    function weightBand(n, cap) {
        const hardCap = typeof cap === 'number' && cap > 0 ? cap : 2000;
        /**
         * @param {import('../types').WeightBand} key @param {string} tone @param {boolean} needsAck
         */
        const band = (key, tone, needsAck) => ({
            key: key, label: t('weight.' + key + '.label'),
            fg: 'var(--' + tone + '-fg)', bg: 'var(--' + tone + '-bg)', bd: 'var(--' + tone + '-bd)', needsAck: needsAck,
            body: t('weight.' + key + '.body', {cap: hardCap})
        });
        if (!Number.isFinite(n) || n < 1) return band('invalid', 'crit', false);
        if (n <= Math.min(500, hardCap)) return band('safe', 'ok', false);
        if (n <= Math.min(1200, hardCap)) return band('heavy', 'warn', false);
        if (n <= hardCap) return band('ceiling', 'crit', true);
        return band('over', 'crit', false);
    }

    /**
     * A file size in the unit an operator reads. One decimal at most: the second
     * would claim a precision a report does not have, its weight moves with what
     * the run found.
     * @param {number} n
     */
    function bytes(n) {
        if (!Number.isFinite(n) || n < 0) return '';
        if (n < 1024) return n + ' ' + t('unit.byte');
        const kb = Math.round(n / 1024);
        // Rounding can carry into the next unit: 1023.5 KiB must read
        // "1.0 MB", never "1024 KB".
        if (kb < 1024) return kb + ' ' + t('unit.kb');
        return decimal((n / (1024 * 1024)).toFixed(1)) + ' ' + t('unit.mb');
    }

    /**
     * A value the way a POSIX shell reads it back, byte for byte.
     *
     * Single quotes and never double: inside double quotes a shell still expands
     * `$`, backticks and backslashes, so a service name carrying one would run as
     * something other than what is displayed. Nothing expands inside single
     * quotes, and the only character needing care is the quote itself, closed and
     * reopened around an escaped one.
     *
     * The bare form is kept for values drawn entirely from the set no shell
     * treats specially, because quoting `order-service` would only make the
     * common line harder to read.
     *
     * @param {string} value
     * @returns {string}
     */
    function shq(value) {
        const text = String(value);
        if (text !== '' && /^[A-Za-z0-9_@%+=:,./-]+$/.test(text)) return text;
        return '\'' + text.replace(/'/g, '\'\\\'\'') + '\'';
    }

    /**
     * The same job for PowerShell, which quotes by a different rule: inside
     * single quotes everything is literal and a quote is doubled, where a POSIX
     * shell has to close, escape and reopen.
     *
     * The bare-word set is narrower than the POSIX one on purpose. A comma is
     * PowerShell's array operator, so `a,b` would arrive as two arguments, and
     * `@` opens a splat or a hash literal. Both are quoted here rather than
     * trusted.
     *
     * @param {unknown} value
     * @returns {string}
     */
    function psq(value) {
        const text = String(value);
        if (text !== '' && /^[A-Za-z0-9_.:/=+-]+$/.test(text)) return text;
        return '\'' + text.replace(/'/g, '\'\'') + '\'';
    }

    /* One entry per shell the launcher can spell a command for. `wrap` is what
       continues a command on the next line: a backslash in a POSIX shell, a
       backtick in PowerShell. */
    const SHELLS = [
        {id: 'posix', label: 'bash / zsh', wrap: '\\', quote: shq},
        {id: 'powershell', label: 'PowerShell', wrap: '`', quote: psq}
    ];

    /**
     * The line that puts a value in an environment variable, in the shell's own
     * syntax. The two differ by more than a keyword: PowerShell assigns into the
     * `env:` drive and wants the spaces around the equals sign.
     *
     * @param {string} shellId
     * @param {string} name
     * @param {string} value
     * @returns {string}
     */
    function exportLine(shellId, name, value) {
        const shell = shellById(shellId);
        return shell.id === 'powershell'
            ? '$env:' + name + ' = ' + shell.quote(value)
            : 'export ' + name + '=' + shell.quote(value);
    }

    function shellById(id) {
        return SHELLS.find(function (shell) {
            return shell.id === id;
        }) || SHELLS[0];
    }

    /**
     * Which shell to spell a command for when the reader has not chosen one.
     * Windows gets PowerShell, which is what its terminal opens with, and
     * everything else gets the POSIX line.
     *
     * @param {string | null | undefined} platform navigator.platform, or the
     *   userAgentData platform, whichever the browser offers
     * @returns {string}
     */
    function defaultShell(platform) {
        // Anchored, not a substring: "Darwin" contains "win", and a caller passing
        // a Node style platform string would have handed macOS a PowerShell line.
        return /^win/i.test(String(platform || '').trim()) ? 'powershell' : 'posix';
    }

    /**
     * A shell id if it names one, null otherwise. shellById always answers with a
     * shell, which is right for spelling a command and wrong for judging whether
     * a remembered value is still one.
     *
     * @param {string | null | undefined} id
     * @returns {string | null}
     */
    function knownShell(id) {
        return SHELLS.some(function (shell) {
            return shell.id === id;
        }) ? String(id) : null;
    }

    /**
     * Whole seconds, the way the Hub writes them into its own invocation. The
     * printed window and the launched one have to be the same window.
     * @param {number} ms
     * @returns {string}
     */
    function isoUtc(ms) {
        return new Date(ms).toISOString().replace(/\.\d{3}Z$/, 'Z');
    }

    /**
     * The request as the engine's own command line.
     *
     * Takes the very object the launcher posts, never the form, so the printed
     * command and the submitted run cannot drift: one shape feeds both, and a
     * field added to one without the other goes missing here rather than wrong.
     * Returns null for a daemon, which takes no arguments at all.
     *
     * The break follows the engine's own examples: the subcommand, the endpoint
     * and the selector on the first line, everything else on the second.
     *
     * @param {import('../types').Source} source
     * @param {Record<string, unknown>} request
     * @returns {string | null}
     */
    function analysisCommand(source, request, shellId) {
        if (!source.engine_subcommand) return null;
        const shell = shellById(shellId);
        const shq = shell.quote;
        const head = ['perf-sentinel ' + source.engine_subcommand, '--endpoint ' + shq(source.base_url)];
        /** @type {string[]} */
        const tail = [];
        if (request['trace_id'] != null) {
            head.push('--trace-id ' + shq(String(request['trace_id'])));
        } else {
            head.push('--service ' + shq(String(request['service'] == null ? '' : request['service'])));
            if (request['from_ms'] != null) {
                tail.push('--from ' + isoUtc(Number(request['from_ms'])));
                tail.push('--to ' + isoUtc(Number(request['to_ms'])));
            } else {
                tail.push('--lookback ' + shq(String(request['lookback'])));
            }
            tail.push('--max-traces ' + String(request['max_traces']));
        }
        if (source.auth_header_name) tail.push('--auth-header-env PERF_SENTINEL_SOURCE_TOKEN');
        // Undotted, and named rather than left to the engine's discovery of
        // `.perf-sentinel.toml`, which is dotted and cwd-only. The Hub hands this
        // file over as a download, and a downloaded file may not keep a leading
        // dot, so asking for the dotted name would ask for one the reader might not
        // have. Naming it also makes a missing file stop the run instead of
        // silently reverting to the defaults the reader just moved away from.
        if (Object.keys(request['detection'] || {}).length > 0) tail.push('-c perf-sentinel.toml');
        return tail.length === 0
            ? head.join(' ')
            : head.join(' ') + ' ' + shell.wrap + '\n  ' + tail.join(' ');
    }

    /**
     * The live view of a daemon in a terminal. `--daemon` sits on `query` and not
     * on `monitor`, so the order is not interchangeable.
     * @param {import('../types').Source} source
     * @returns {string}
     */
    /**
     * `--daemon` belongs to the parent `query`, `--refresh` to `monitor`, so the
     * order is not free. The interval is the one the row is already re-reading
     * on: a reader who slowed this screen down means it, and the command they
     * copy should not contradict the screen they copied it from.
     *
     * @param {any} source
     * @param {number | undefined} refreshSeconds omitted, or 0, leaves the engine's own default
     * @param {string} shellId
     * @returns {string}
     */
    function monitorCommand(source, refreshSeconds, shellId) {
        const command = 'perf-sentinel query --daemon '
            + shellById(shellId).quote(source.base_url) + ' monitor';
        return Number.isInteger(refreshSeconds) && refreshSeconds > 0
            ? command + ' --refresh ' + refreshSeconds
            : command;
    }

    /**
     * The line printed under the monitor command of a daemon behind a header. The
     * monitor sends `PERF_SENTINEL_DAEMON_API_KEY` as `X-API-Key` and nothing else,
     * so only that header gets the key hint, as prose since no shell quotes it. Any
     * other, an Ingress's own for one, is named as a header the command cannot send.
     *
     * @param {any} source
     * @returns {string | null}
     */
    function monitorKeyHint(source) {
        const header = source.auth_header_name;
        if (!header) return null;
        return header.toLowerCase() === 'x-api-key'
            ? t('daemon.monitor.api_key')
            : t('daemon.monitor.header', {header: header});
    }

    /**
     * The overridden thresholds as the file `-c` expects. Only the ones a run
     * actually changed: a value equal to the engine's own default is dropped
     * before it reaches here, so every key present is a real departure. A
     * choice rides as a string and is quoted, a threshold as a number.
     * @param {Record<string, number|string>} detection
     * @returns {string}
     */
    function detectionToml(detection) {
        return ['[detection]'].concat(Object.keys(detection).sort().map(function (name) {
            const value = detection[name];
            // TOML has no bare words: a choice is a quoted string or a parse error.
            return name + ' = ' + (typeof value === 'string' ? JSON.stringify(value) : value);
        })).join('\n');
    }

    /** True when a value had to be quoted, so the block can name the shell. */
    function quotedForShell(command) {
        return command.indexOf('\'') >= 0;
    }

    /**
     * The state a light refresh yields, mirroring DaemonView.Classify with the
     * hints of the page's last full read: a status-only body carries the gauges
     * but not the daemon's hints, and once a minute a full read re-syncs both.
     *
     * @param {{traces: any, analysis_queue: any, findings: any}} view
     * @param {number} warningCount
     * @returns {string}
     */
    function lightState(view, warningCount) {
        const gauges = [view.traces, view.analysis_queue, view.findings];
        if (gauges.some(function (g) {
            return g && g.at_capacity;
        })) return 'near_capacity';
        if (warningCount > 0) return 'advised';
        return gauges.every(function (g) {
            return !g || g.pct === null;
        }) ? 'unknown' : 'ok';
    }

    /**
     * The folds worth remembering: the open ones. A closed fold is the default,
     * so storing it would only grow the record every time a reader tidies up
     * after themselves, and leave the names of sources that no longer exist.
     *
     * @param {Record<string, boolean>} folds
     * @returns {Record<string, true>}
     */
    function openFolds(folds) {
        /** @type {Record<string, true>} */
        const open = {};
        Object.keys(folds || {}).forEach(function (key) {
            if (folds[key] === true) open[key] = true;
        });
        return open;
    }

    /** The settings groups' fold keys named each group by its English heading before the French launcher. */
    const OLD_DAEMON_GROUPS = {
        '// declared': 'declared',
        '// ingestion and memory': 'ingestion',
        '// what it keeps for readers': 'readers',
        '// metrics labels': 'labels',
        '// listeners': 'listeners',
        '// sub-systems': 'subsystems',
        '// correlation': 'correlation'
    };

    /**
     * The open settings groups, with a `<source>|<old heading>` key renamed to its group key, so
     * an upgrade keeps them open and the next save drops the old key.
     * @param {Record<string, boolean>} folds
     * @returns {Record<string, true>}
     */
    function daemonGroupFolds(folds) {
        /** @type {Record<string, true>} */
        const open = {};
        Object.keys(openFolds(folds)).forEach(function (key) {
            const bar = key.lastIndexOf('|');
            const group = OLD_DAEMON_GROUPS[key.slice(bar + 1)];
            open[group ? key.slice(0, bar + 1) + group : key] = true;
        });
        return open;
    }

    /* The engine's own advisor line: at 90 % of a cap the daemon starts saying so
       in its hints, and the row's verdict turns to near capacity. The 75 % step
       below it is the Hub's, a heads-up before the daemon complains rather than a
       second opinion about when it should. */
    const GAUGE_WARN_PCT = 75;
    const GAUGE_CRIT_PCT = 90;

    /**
     * The tone a gauge's own figure takes, or null to leave it in the plain text
     * colour. Only a known percentage is toned: a gauge with no published cap
     * says nothing about how close to one it is.
     *
     * @param {number | null | undefined} pct
     * @returns {'crit' | 'warn' | null}
     */
    function gaugeTone(pct) {
        if (typeof pct !== 'number' || !isFinite(pct)) return null;
        if (pct >= GAUGE_CRIT_PCT) return 'crit';
        if (pct >= GAUGE_WARN_PCT) return 'warn';
        return null;
    }

    /**
     * How far a gauge moved between two reads, or null when there is nothing to
     * show: no earlier reading, a value either side is unknown, or one that did
     * not move at all.
     *
     * @param {any} before
     * @param {any} after
     * @returns {number | null}
     */
    function gaugeMove(before, after) {
        const from = before ? before.value : null;
        const to = after ? after.value : null;
        if (typeof from !== 'number' || typeof to !== 'number' || from === to) return null;
        return to - from;
    }

    const ENGINE_REPOSITORY = 'https://github.com/Institut-du-Numerique-Responsable/perf-sentinel';
    const HUB_REPOSITORY = 'https://github.com/Institut-du-Numerique-Responsable/PerfSentinelHub';
    /* Every chart version with the engine version it ships, which a release tag
       cannot give: a chart-only fix bumps the chart and not the appVersion. */
    const CHART_PAGE = 'https://artifacthub.io/packages/helm/perf-sentinel/perf-sentinel';
    /* Not a link. safeHttpsHref would refuse the scheme, and it is a coordinate
       to paste into helm rather than a page to open. */
    const CHART_COORDINATE = 'oci://ghcr.io/institut-du-numerique-responsable/charts/perf-sentinel';

    /**
     * Where to get the engine a printed command needs. Pinned to the version
     * this Hub runs, since that is the one the command is spelled for. Anything
     * that is not a plain version lands on the release list instead of building
     * a URL out of it: the string comes from a binary's own --version.
     *
     * @param {string | null} version
     * @returns {string}
     */
    function releaseUrl(version) {
        return /^[0-9][0-9A-Za-z.+-]{0,63}$/.test(String(version || ''))
            ? ENGINE_REPOSITORY + '/releases/tag/v' + version
            : ENGINE_REPOSITORY + '/releases';
    }

    /**
     * Where to get a newer Hub. Always the release list: a tag URL would have to
     * be built from a version, and the newest one is what the reader wants.
     *
     * @returns {string}
     */
    function hubReleaseUrl() {
        return HUB_REPOSITORY + '/releases';
    }

    /**
     * Whether a newer release exists, given what is running and what the Hub last
     * read from GitHub.
     *
     * Null covers every case where there is nothing to say, and they are not the
     * same case: the check is off or has not run (latest null), the product is
     * not configured at all (current null), or the two match. A caller that
     * treated null as "up to date" would say so about a Hub that never asked.
     *
     * @param {string | null} current
     * @param {string | null} latest
     * @returns {{latest: string} | null}
     */
    function updateState(current, latest) {
        if (!current || !latest) return null;
        // Behind only. A build ahead of the newest release is a pre-release or a
        // local build, and telling its operator to downgrade would be wrong.
        return vcmp(current, latest) < 0 ? {latest: latest} : null;
    }

    /**
     * The last view a light refresh can merge onto, or null when there is none:
     * undefined before the first read, the "loading" sentinel during it, an error
     * body, or a light body that never carried the rest. Null is the caller's
     * signal to read in full rather than light.
     *
     * @param {any} view
     * @returns {any}
     */
    function mergeableView(view) {
        return view && typeof view === 'object' && !view.error_code && Array.isArray(view.warnings)
            ? view
            : null;
    }

    /**
     * What the next re-read of an open row should cost.
     *
     *   full   the whole view: the only read that renders a panel, so it is what
     *          a row starts with and what it returns to once a minute
     *   light  the gauges alone, laid over the view already on screen
     *   probe  the gauges alone again, but to find out whether a row that failed
     *          can be read at all: an answer is followed by a full read
     *
     * @param {any} previous the view currently on screen, if any
     * @param {number} sinceFullMs since the last full read
     * @param {number} everyMs how often a full read is due
     * @returns {'full' | 'light' | 'probe'}
     */
    function refreshPlan(previous, sinceFullMs, everyMs) {
        if (mergeableView(previous)) return sinceFullMs >= everyMs ? 'full' : 'light';
        // Only a view that failed is worth probing. Everything else, nothing read
        // yet or a read in flight, starts over with the read that renders.
        return previous && typeof previous === 'object' && previous.error_code ? 'probe' : 'full';
    }

    /**
     * A light body laid over the last full view: the gauges and the version are
     * the light read's, the settings and the daemon's hints stay the full read's,
     * and the state is derived from both.
     *
     * @param {any} previous a view mergeableView returned
     * @param {any} light
     * @returns {any}
     */
    function mergeLight(previous, light) {
        return Object.assign({}, previous, {
            observed_at_ms: light.observed_at_ms,
            version: light.version,
            uptime_seconds: light.uptime_seconds,
            traces: light.traces,
            analysis_queue: light.analysis_queue,
            findings: light.findings,
            state: lightState(light, previous.warnings.filter(function (hint) {
                // DaemonView.AdvisingCount's set: the other kinds describe the daemon.
                return hint.kind === 'ingestion_drops' || hint.kind === 'tuning';
            }).length)
        });
    }

    /**
     * The daemon's closed set of incident kinds, as the incidents screen labels
     * them. Anything else was already folded to `other` by the Hub's parser.
     * @type {Record<string, string>}
     */
    const INCIDENT_KIND_LABEL = textTable('incident.kind.',
        ['oom_kill', 'memory_saturation', 'restart', 'deploy', 'other']);

    /**
     * The engine's twelve finding types, worded as its own dashboard words them.
     * Kept as a display map and nothing else: `finding_type` is a stored column,
     * an index key and a public query parameter of `/api/findings`, so the raw
     * string has to survive everywhere but the cell a person reads.
     *
     * The fallback is load-bearing rather than defensive. The Hub accepts any
     * string on ingest, so a newer engine or another producer can send a
     * thirteenth type, and it belongs on screen as itself.
     * @type {Record<string, string>}
     */
    const FINDING_TYPE_LABEL = textTable('finding.type.', ['n_plus_one_sql', 'n_plus_one_http',
        'n_plus_one_messaging', 'redundant_sql', 'redundant_http', 'slow_sql', 'slow_http', 'slow_messaging',
        'excessive_fanout', 'chatty_service', 'pool_saturation', 'serialized_calls']);

    /**
     * The daemon's own reading of `oldest_finding_ms`: at or below the window's
     * start the ring still reached the whole window, above it part of the window
     * had already been evicted when the incident was frozen, and absent means the
     * ring was empty. The Hub publishes the same verdict as `capture`, this is the
     * page's copy of the rule for a row it has not asked the Hub about.
     * @param {{oldest_finding_ms?: number | null, window_from_ms: number}} incident
     * @returns {'complete' | 'partial' | 'empty'}
     */
    function incidentCapture(incident) {
        if (incident.oldest_finding_ms == null) return 'empty';
        return incident.oldest_finding_ms <= incident.window_from_ms ? 'complete' : 'partial';
    }

    /**
     * The fold behind which the incident filters an operator reaches for less
     * often wait, named with how many of them are set, so a folded filter is
     * never silently in force.
     * @param {{namespace: string, environment: string, source_id: string}} filter
     * @returns {string}
     */
    function moreFiltersLabel(filter) {
        const set = ['namespace', 'environment', 'source_id'].filter(function (key) {
            return filter[key] !== '';
        }).length;
        return set === 0 ? t('incident.filter.more') : t('incident.filter.more_set.' + plural(set), {count: set});
    }

    const SEVERITY_RANK = {critical: 3, warning: 2, info: 1};

    /**
     * The normalized SQL or HTTP template a finding was raised on, the one thing
     * that tells apart two findings of one type on one endpoint.
     * @param {{finding?: any}} row
     * @returns {string}
     */
    function findingTemplate(row) {
        return String(((row.finding || {}).pattern || {}).template || '');
    }

    /**
     * The avoidable I/O operations a frozen finding stands for, traces seen times
     * extra operations per trace: the findings dashboard's "Est. impact (ops)".
     * Null when the daemon did not score green impact.
     * @param {{seen_count?: number, finding?: any}} row
     * @returns {number | null}
     */
    function findingImpact(row) {
        const perTrace = ((row.finding || {}).green_impact || {}).estimated_extra_io_ops;
        if (perTrace == null || row.seen_count == null) return null;
        return row.seen_count * perTrace;
    }

    /**
     * Whether a frozen finding matches what the operator typed above the table,
     * on its endpoint or its template, ignoring case.
     * @param {{finding?: any}} row
     * @param {string} query
     * @returns {boolean}
     */
    function findingMatches(row, query) {
        const needle = String(query || '').trim().toLowerCase();
        if (needle === '') return true;
        return (((row.finding || {}).source_endpoint || '') + '\n' + findingTemplate(row))
            .toLowerCase().indexOf(needle) >= 0;
    }

    /**
     * The daemon-side ack, for a finding this Hub cannot acknowledge itself. The
     * reason is left for the operator to write, the flag order is the CLI's own:
     * `--daemon` belongs to `ack`, before the subcommand.
     * @param {{base_url: string}} source
     * @param {string} signature
     * @param {string} shellId
     * @returns {string}
     */
    function ackCommand(source, signature, shellId) {
        const quote = shellById(shellId).quote;
        return 'perf-sentinel ack --daemon ' + quote(source.base_url) + ' create --signature '
            + quote(signature) + ' --reason ' + quote(t('ack.command.reason'));
    }

    /**
     * An incident's frozen findings in the order a column asks for. Ties fall
     * back to the default reading order, worst and most seen first, so equal
     * rows never shuffle between two renders.
     * @param {Array<{seen_count?: number, first_seen_ms: number, finding?: any}>} rows
     * @param {'type' | 'severity' | 'impact' | 'endpoint' | 'template' | 'seen' | 'first'} key
     * @param {'asc' | 'desc'} dir
     * @returns {Array<any>}
     */
    function sortFindings(rows, key, dir) {
        const value = {
            type: function (row) {
                const type = (row.finding || {}).type || '';
                return FINDING_TYPE_LABEL[type] || type;
            },
            severity: function (row) {
                return SEVERITY_RANK[(row.finding || {}).severity] || 0;
            },
            // Unscored below a scored zero, so a descending sort leads with what was measured.
            impact: function (row) {
                const impact = findingImpact(row);
                return impact == null ? -1 : impact;
            },
            endpoint: function (row) {
                return (row.finding || {}).source_endpoint || '';
            },
            template: function (row) {
                return findingTemplate(row);
            },
            seen: function (row) {
                return row.seen_count || 0;
            },
            first: function (row) {
                return row.first_seen_ms;
            }
        };
        const compare = function (name, a, b) {
            const x = value[name](a);
            const y = value[name](b);
            return typeof x === 'string' ? x.localeCompare(y) : x - y;
        };
        const sign = dir === 'asc' ? 1 : -1;
        return rows.slice().sort(function (a, b) {
            return sign * compare(key, a, b)
                || -compare('severity', a, b) || -compare('seen', a, b)
                || compare('endpoint', a, b) || compare('first', a, b);
        });
    }

    /**
     * How a frozen finding's first sighting reads against its incident. An OOM
     * kill ends in a restart and a deploy is its own event, so "after the
     * restart" is said only where a restart happened.
     * @param {'before' | 'after'} phase
     * @param {string} kind
     * @returns {string}
     */
    function phaseLabel(phase, kind) {
        if (phase !== 'after') return t('incident.phase.before');
        if (kind === 'restart' || kind === 'oom_kill') return t('incident.phase.after_restart');
        return t(kind === 'deploy' ? 'incident.phase.after_deploy' : 'incident.phase.after');
    }

    /**
     * Whether a frozen finding was already burning before the incident or fired
     * only after it. A finding is stamped when its trace is analysed, one TTL
     * after its last span, so a stamp past `at_ms` belongs to the replacement.
     * @param {{first_seen_ms: number}} finding
     * @param {{at_ms: number}} incident
     * @returns {'before' | 'after'}
     */
    function findingPhase(finding, incident) {
        return finding.first_seen_ms > incident.at_ms ? 'after' : 'before';
    }

    /**
     * What the last incidents read of one daemon came to, in the words the
     * incidents screen uses under its table. `ok` says nothing beyond the age,
     * which is already there.
     * @type {Record<string, string>}
     */
    const INCIDENT_READ_STATE = textTable('incident.read.', ['absent', 'unauthorized', 'error']);

    /**
     * How the service and namespace filters of the incidents screen show. They
     * list what a read returns, so they wait, disabled, for the first one, the
     * namespace filter holding its place so the line does not shift when it
     * lands. After any read, even one that found nothing, they stay enabled
     * through later refreshes, and the namespace filter shows only once a row
     * has carried a namespace: a select over nothing would promise a column the
     * fleet has not filled.
     * @param {boolean} read whether a read of the fleet has completed
     * @param {string[]} namespaces the namespaces the reads have seen
     * @returns {{pending: boolean, namespace: boolean}}
     */
    function incidentFilterState(read, namespaces) {
        return {pending: !read, namespace: !read || namespaces.length > 0};
    }

    /**
     * How fresh the Hub's copy of one daemon's incidents is. The screen lists
     * one of these per daemon, so a quiet fleet reads differently from a stale
     * copy: an hour-old read of nothing is not the same answer as no read.
     * @param {{name: string, incidents_read_ms?: number | null, incidents_state?: string | null}} source
     * @param {number} nowMs
     * @returns {string}
     */
    function incidentsCopy(source, nowMs) {
        if (source.incidents_read_ms == null) return t('incident.copy.never', {name: source.name});
        const age = t('incident.copy.read',
            {name: source.name, duration: dur(Math.max(0, nowMs - source.incidents_read_ms))});
        const state = INCIDENT_READ_STATE[source.incidents_state || ''];
        return state ? age + ', ' + state : age;
    }

    /**
     * The route that opens New analysis on an incident's window. The end is held
     * at now: an incident younger than two TTLs still has a `window_to_ms` in the
     * future, and the Hub refuses a window that ends there.
     * @param {{id: string, service: string, window_from_ms: number, window_to_ms: number}} incident
     * @param {number} nowMs
     * @returns {string}
     */
    function incidentHandoffHash(incident, nowMs) {
        return '#/new?from=' + incident.window_from_ms
            + '&to=' + Math.min(incident.window_to_ms, nowMs)
            + '&service=' + encodeURIComponent(incident.service)
            + '&incident=' + encodeURIComponent(incident.id);
    }

    /**
     * What a `#/new?from=…&to=…&service=…` hash carries for the form, or null
     * when it carries nothing the form can take: both bounds numbers with the
     * start before the end, a service name, and the end no later than now, held
     * there rather than refused since a shared link ages. Any other route, and
     * the bare `#/new` the tab points at, read as null.
     * @param {string | null | undefined} hash
     * @param {number} nowMs
     * @returns {{fromMs: number, toMs: number, service: string, incidentId: string, pairKey: string} | null}
     */
    function readHandoff(hash, nowMs) {
        const text = String(hash || '');
        if (text.indexOf('#/new?') !== 0) return null;
        const params = new URLSearchParams(text.slice('#/new?'.length));
        const number = function (key) {
            const raw = params.get(key);
            return raw === null || raw.trim() === '' ? NaN : Number(raw);
        };
        const fromMs = number('from');
        const toMs = Math.min(number('to'), nowMs);
        const service = (params.get('service') || '').trim();
        if (!Number.isFinite(fromMs) || !Number.isFinite(toMs) || fromMs >= toMs || service === '') return null;
        return {
            fromMs: fromMs,
            toMs: toMs,
            service: service,
            incidentId: params.get('incident') || '',
            pairKey: params.get('pair') || ''
        };
    }

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
        if (ms < 1000) return Math.round(ms) + ' ' + t('unit.ms');
        if (ms < 60000) return decimal((ms / 1000).toFixed(1)) + ' ' + t('unit.s');
        return dur(ms);
    }

    /**
     * A closed episode's span: its start whole, its end as a clock when it falls on the same day.
     * @param {number} startMs
     * @param {number} endMs
     * @returns {string}
     */
    function episodeSpan(startMs, endMs) {
        const sameDay = dtHuman(endMs).slice(0, 10) === dtHuman(startMs).slice(0, 10);
        return dateTime(startMs) + ' → ' + (sameDay ? time(endMs) : dateTime(endMs));
    }

    /**
     * The cache key of an incident's correlations: the window it was read for, so an incident
     * that ended since, or a wider read, asks again instead of serving the first answer.
     * @param {string} incidentKey
     * @param {number} fromMs
     * @param {number} toMs
     * @returns {string}
     */
    function correlationWindowKey(incidentKey, fromMs, toMs) {
        return incidentKey + '@' + fromMs + '-' + toMs;
    }

    /**
     * A pair's accessible name: both finding types and both services, so two pairs between
     * the same services read apart.
     * @param {{source: {finding_type: string, service: string}, target: {finding_type: string, service: string}}} episode
     * @returns {string}
     */
    function pairLabel(episode) {
        const side = function (end) {
            return t('correlation.pair.side',
                {type: FINDING_TYPE_LABEL[end.finding_type] || end.finding_type, service: end.service});
        };
        return t('correlation.pair.label', {first: side(episode.source), then: side(episode.target)});
    }

    /**
     * What an ended episode says beside its span: `stale` when the Hub ended it because its
     * daemon went quiet, nothing when the daemon closed it.
     * @param {{ended_by?: string | null}} episode
     * @returns {string}
     */
    function episodeEndNote(episode) {
        return episode.ended_by === 'stale' ? t('correlation.ended_stale') : '';
    }

    /**
     * The route of the ack page. The environment and the source only decide
     * which rows start checked, so a link without them is whole. A blank one is
     * left out: Grafana sends a single space for the All choice of a variable.
     * Both travel trimmed, since a padded one would match no row.
     * @param {string} signature
     * @param {string | null | undefined} sourceId
     * @param {string | null | undefined} [environment]
     * @returns {string}
     */
    function ackRouteHash(signature, sourceId, environment) {
        const part = function (key, value) {
            const clean = String(value || '').trim();
            return clean === '' ? '' : '&' + key + '=' + encodeURIComponent(clean);
        };
        return '#/ack?signature=' + encodeURIComponent(signature)
            + part('environment', environment) + part('source_id', sourceId);
    }

    /**
     * The ack route for a `/?ack=<signature>&environment=…&source_id=…` address,
     * the last two optional, or null without an `ack` parameter. The link
     * arrives as a query because a hash is lost when the identity provider asks
     * for a password on the way in. An empty value still routes, so the page can
     * say the link is incomplete.
     * @param {string | null | undefined} search
     * @returns {string | null}
     */
    function ackEntryHash(search) {
        const params = new URLSearchParams(String(search || ''));
        return params.has('ack')
            ? ackRouteHash(params.get('ack') || '', params.get('source_id'), params.get('environment'))
            : null;
    }

    /**
     * What a `#/ack?signature=…` hash names, or null when the page cannot act
     * on it. The signature's bounds are the Hub's own, 1,024 characters and no
     * control character. A blank signature is refused here because
     * `/api/findings` reads one as no filter and would answer with a finding the
     * link never named. A blank source or environment is no context, read as
     * null, while an environment past 256 characters or carrying a control
     * character is a link nobody meant to write.
     * @param {string | null | undefined} hash
     * @returns {{signature: string, sourceId: string | null, environment: string | null} | null}
     */
    function readAckRoute(hash) {
        const text = String(hash || '');
        if (text.indexOf('#/ack?') !== 0) return null;
        const params = new URLSearchParams(text.slice('#/ack?'.length));
        const named = function (key) {
            return (params.get(key) || '').trim() || null;
        };
        const signature = params.get('signature') || '';
        if (signature.trim() === '' || signature.length > 1024 || /\p{Cc}/u.test(signature)) return null;
        const environment = named('environment');
        if (environment !== null && (environment.length > 256 || /\p{Cc}/u.test(environment))) return null;
        return {signature: signature, sourceId: named('source_id'), environment: environment};
    }

    /**
     * The ack reads whose listing the Hub mirrors acks from. Any other state
     * leaves `acks[]` silent about that source, which proves nothing.
     */
    const ACK_STATES_KNOWN = ['ok', 'truncated'];

    /**
     * One row per source that carries the finding, with what the page can do
     * there. An ack lives in one daemon's own store, so each source is acked and
     * revoked by itself, and an ack hides a finding, so the link's context sets
     * the default ticks: the source it names, else the sources of the environment
     * it names, else a source that is alone in taking an action. Every other row
     * stays the reader's to tick.
     *
     * A relaying source whose ack state is unknown takes `either` action. A
     * daemon below 0.24.0 never has a listing and one failed read proves
     * nothing, while the relay would still answer, so the daemon is left to
     * refuse the button that does not apply.
     * @param {{sources?: Array<{id: string, name: string, environment: string,
     *   producer_version?: string | null}>,
     *   acks?: Array<{source_id: string, source: string}>}} finding
     * @param {Array<import('../types').Source> | null | undefined} sources
     * @param {{sourceId?: string | null, environment?: string | null} | null | undefined} scope
     * @returns {Array<{id: string, name: string, environment: string, relay: boolean,
     *   ack: {source_id: string, source: string} | null, action: 'ack' | 'revoke' | 'either' | 'none',
     *   checked: boolean, note: string | null}>}
     */
    function ackRows(finding, sources, scope) {
        const sourceId = (scope && scope.sourceId) || null;
        const environment = (scope && scope.environment) || null;
        // A source that only counted the finding holds no copy to ack or revoke.
        const copies = (finding.sources || []).filter(function (carrier) {
            return carrier.producer_version !== null;
        });
        const rows = copies.map(function (carrier) {
            const source = (sources || []).find(function (candidate) {
                return candidate.id === carrier.id;
            });
            const ack = (finding.acks || []).find(function (candidate) {
                return candidate.source_id === carrier.id;
            }) || null;
            const relay = Boolean(source && source.ack_relay);
            let action = 'none';
            let note = null;
            if (!source) note = t('ack.note.unconfigured');
            else if (!relay) note = t('ack.note.no_credential');
            else if (ack && ack.source === 'daemon') action = 'revoke';
            else if (ack) note = t('ack.note.ci_baseline');
            else if (ACK_STATES_KNOWN.indexOf(source.acks_state) < 0) {
                action = 'either';
                note = t('ack.note.unknown_state', {state: source.acks_state || t('ack.note.never_read')});
            } else action = 'ack';
            return {
                id: carrier.id,
                name: (source || carrier).name,
                environment: (source || carrier).environment,
                relay: relay,
                ack: ack,
                action: /** @type {'ack' | 'revoke' | 'either' | 'none'} */ (action),
                checked: false,
                note: note
            };
        });
        const actionable = rows.filter(function (row) {
            return row.action !== 'none';
        });
        actionable.forEach(function (row) {
            if (sourceId) row.checked = row.id === sourceId;
            else if (environment) row.checked = row.environment === environment;
            else row.checked = actionable.length === 1;
        });
        return rows;
    }

    /**
     * A detection count as the page shows it: `?` when no day of the window is
     * known, `at least N` when a reason keeps it short, `N` when it is whole.
     * @param {number | null | undefined} detections
     * @param {boolean | null | undefined} complete
     * @returns {string}
     */
    function formatDetections(detections, complete) {
        if (detections === null || detections === undefined) return '?';
        const count = num(detections);
        return complete === false ? '\u2265 ' + count : count;
    }

    /**
     * The from and to of a windowed read counting `days` whole UTC days, today the last.
     * The Hub counts every day from the from day to the to day, so `now - days` would
     * count one day more.
     * @param {number} nowMs
     * @param {number} days
     * @returns {{from: number, to: number}}
     */
    function detectionWindow(nowMs, days) {
        const dayMs = 86400000;
        return {from: (Math.floor(nowMs / dayMs) - days + 1) * dayMs, to: nowMs};
    }

    const GAP_REASONS = ['uncovered_days', 'open_day', 'restart', 'sampling', 'shed', 'ingest_rejected', 'overflow',
        'multi_instance'];
    const GAP_LABEL = textTable('gap.tech.', GAP_REASONS);

    /** The same reasons in a Management reader's words, as RecapText's twin. */
    const GAP_LABEL_PILOTAGE = textTable('gap.pilotage.', GAP_REASONS);

    /**
     * Why a detection count falls short, in a few words. A reason a newer
     * engine adds is returned as it was sent, or as a plain stand-in for Management.
     * @param {string} reason
     * @param {string} [view] `pilotage` for the Management words
     * @returns {string}
     */
    function gapLabel(reason, view) {
        const own = Object.prototype.hasOwnProperty;
        if (view === 'pilotage') {
            return own.call(GAP_LABEL_PILOTAGE, reason) ? GAP_LABEL_PILOTAGE[reason] : t('gap.pilotage.other');
        }
        return own.call(GAP_LABEL, reason) ? GAP_LABEL[reason] : reason;
    }

    /**
     * The `expires_at` of an ack that lasts through the day a date input holds:
     * the last second of that day in UTC, null for no date, which is a permanent
     * ack. A day already over in UTC throws, since the Hub refuses an expiry
     * that is not ahead, and the message is the sentence the page shows.
     * @param {string | null | undefined} dateValue
     * @param {number} nowMs
     * @returns {string | null}
     */
    function ackExpiry(dateValue, nowMs) {
        const day = String(dateValue || '');
        if (day === '') return null;
        const endMs = Date.parse(day + 'T23:59:59Z');
        // The engine rolls the 31st of February over rather than refusing it.
        if (!Number.isFinite(endMs) || isoUtc(endMs).slice(0, 10) !== day) {
            throw new RangeError(t('ack.expiry.not_a_date'));
        }
        if (endMs <= nowMs) throw new RangeError(t('ack.expiry.past'));
        return isoUtc(endMs);
    }

    /**
     * What the date input under an ack amounts to, in UTC, beside the date the
     * browser shows in its own locale.
     * @param {string | null | undefined} dateValue
     * @param {number} nowMs
     * @returns {string}
     */
    function ackExpiryLine(dateValue, nowMs) {
        try {
            const end = ackExpiry(dateValue, nowMs);
            return end === null ? t('ack.expiry.permanent') : t('ack.expiry.until', {day: end.slice(0, 10)});
        } catch (error) {
            return error.message;
        }
    }

    /**
     * Whether a row is checked: the reader's own tick when they made one for the
     * action the row offers now, what ackRows says otherwise. A tick made on an
     * ack says nothing once the row reads Revoke, while a source the reader left
     * out stays out however often the rows are read again. `either` overlaps
     * both actions, so a tick made on it or read against it still speaks: an ack
     * state that turns known or unknown between two reads must not tick a row
     * the reader left out.
     * @param {{id: string, action: string, checked: boolean}} row
     * @param {Record<string, {action: string, checked: boolean}> | null | undefined} ticks
     * @returns {boolean}
     */
    function ackChecked(row, ticks) {
        if (row.action === 'none') return false;
        const own = (ticks || {})[row.id];
        const speaks = own && (own.action === row.action || own.action === 'either' || row.action === 'either');
        return speaks ? own.checked : row.checked;
    }

    /**
     * @param {number} count
     * @returns {string}
     */
    function sourceCount(count) {
        return t('ack.sources.' + plural(count), {count: count});
    }

    /**
     * What the two buttons would do with the form as it stands. `blocker` is
     * what keeps Acknowledge dead, while a revoke needs a checked row and nothing
     * else. `blocked` is true only when neither button can be pressed, so the
     * sentence never reads as a refusal beside a button that is ready. A checked
     * row that takes `either` action counts for both buttons.
     * @param {Array<{id: string, action: string, checked: boolean}>} rows
     * @param {Record<string, {action: string, checked: boolean}> | null | undefined} ticks
     * @param {string | null | undefined} reason
     * @param {string | null | undefined} expiryValue
     * @param {number} nowMs
     * @returns {{ack: Array<{id: string, action: string, checked: boolean}>,
     *   revoke: Array<{id: string, action: string, checked: boolean}>, expiresAt: string | null,
     *   blocker: string | null, blocked: boolean, sentence: string}}
     */
    function ackPlan(rows, ticks, reason, expiryValue, nowMs) {
        const checked = function (action) {
            return rows.filter(function (row) {
                return (row.action === action || row.action === 'either') && ackChecked(row, ticks);
            });
        };
        const ack = checked('ack');
        const revoke = checked('revoke');
        let expiresAt = null;
        let blocker = null;
        if (ack.length === 0) blocker = t('ack.plan.no_ack_source');
        else if (String(reason || '').trim() === '') blocker = t('ack.plan.no_reason');
        else {
            try {
                expiresAt = ackExpiry(expiryValue, nowMs);
            } catch (error) {
                blocker = String(error.message);
            }
        }
        const sentences = [];
        if (ack.length > 0) {
            sentences.push(blocker || (expiresAt
                ? t('ack.plan.ack_until', {sources: sourceCount(ack.length), expires: expiresAt})
                : t('ack.plan.ack_permanent', {sources: sourceCount(ack.length)})));
        }
        // The Hub mirrors no ack from a row of unknown state, so it claims none there.
        const unsure = revoke.some(function (row) {
            return row.action === 'either';
        });
        if (revoke.length > 0) {
            sentences.push(t(unsure ? 'ack.plan.revoke_unsure' : 'ack.plan.revoke',
                {sources: sourceCount(revoke.length)}));
        }
        return {
            ack: ack,
            revoke: revoke,
            expiresAt: expiresAt,
            blocker: blocker,
            blocked: blocker !== null && revoke.length === 0,
            sentence: sentences.join(' ') || t('ack.plan.none')
        };
    }

    /**
     * The JSON one relay request carries. The Hub names the caller itself, and a
     * revoke names the finding and nothing else.
     * @param {'ack' | 'revoke'} action
     * @param {string} signature
     * @param {string | null | undefined} reason
     * @param {string | null | undefined} expiresAt
     * @returns {{signature: string, reason?: string, expires_at?: string}}
     */
    function ackBody(action, signature, reason, expiresAt) {
        if (action === 'revoke') return {signature: signature};
        /** @type {{signature: string, reason: string, expires_at?: string}} */
        const body = {signature: signature, reason: String(reason || '').trim()};
        if (expiresAt) body.expires_at = expiresAt;
        return body;
    }

    /**
     * The refusals the Hub sends with no body, plus 0 for a request that never
     * got an answer. Every other refusal carries its own `detail`.
     * @type {Record<number, string>}
     */
    const ACK_REFUSALS = textTable('ack.refusal.', ['0', '413', '503']);

    /**
     * One line per source a submit wrote to, in the order it wrote. `action` is
     * the button pressed, never `either`: on a row of unknown ack state the
     * relay's `detail` carries the daemon's refusal of the button that did not apply.
     * @param {Array<{name: string, action: 'ack' | 'revoke', status: number, detail?: string | null,
     *     code?: string | null}>} results
     * @returns {Array<{ok: boolean, text: string}>}
     */
    function ackSummary(results) {
        return results.map(function (result) {
            const ok = result.status >= 200 && result.status < 300;
            const said = ok
                ? t(result.action === 'revoke' ? 'ack.result.revoked' : 'ack.result.acknowledged')
                : (result.detail ? problemText(result, result.status) : null) || ACK_REFUSALS[result.status]
                || t('ack.result.refused', {status: result.status});
            return {ok: ok, text: t('ack.result.line', {name: result.name, said: said})};
        });
    }

    /**
     * What the shell says when `/api/status` fails. Hub:Access answers 403 to a
     * signed-in caller no grant reaches: the Hub is up, and only an administrator
     * can change that. Any other failure is the Hub itself, and null leaves the
     * page to its unreachable banner.
     * @param {number | undefined} status
     * @returns {string | null}
     */
    function shellBanner(status) {
        return status === 403 ? t('access.no_grant') : null;
    }

    /**
     * Whether a source's own list allows a permission. A source without a list is
     * a Hub with access off, where nothing is withheld.
     * @param {{permissions?: string[] | null}} source
     * @param {string} permission
     * @returns {boolean}
     */
    function permits(source, permission) {
        return !source.permissions || source.permissions.indexOf(permission) >= 0;
    }

    /**
     * A project pattern as the access page writes it: only the parts it sets.
     * @param {{source?: string | null, service?: string | null, attribute?: string | null}} pattern
     * @returns {string}
     */
    function describePattern(pattern) {
        return [['source', pattern.source], ['service', pattern.service], ['attribute', pattern.attribute]]
            .filter(part => part[1] != null)
            .map(part => part[0] + ' ' + part[1])
            .join(' · ');
    }

    // ------------------------------------------------------------ disclosures

    const DAY_MS = 86400000;
    // What an official report needs, the engine's MIN_PERIOD_COVERAGE_FOR_OFFICIAL.
    const OFFICIAL_COVERAGE = 0.75;
    const RAIL_QUARTERS = 12;

    /** The 16-hex id a disclosure route names, or null. */
    function readDisclosureId(hash) {
        const match = /^#\/disclosures\/([0-9a-f]{16})$/.exec(hash || '');
        return match ? match[1] : null;
    }

    function quarterOf(year, month) {
        return (year * 4) + Math.floor((month - 1) / 3);
    }

    function railCell(quarter, items, current, nowMs) {
        const year = Math.floor(quarter / 4);
        const cell = {
            label: t('period.quarter', {year: year, quarter: (quarter % 4) + 1}),
            state: 'missing',
            level: null,
            id: null,
            daysLeft: null
        };
        const live = items.find(function (item) {
            return !item.withdrawn && item.confidentiality !== 'internal';
        }) || items.find(function (item) {
            return !item.withdrawn;
        });
        const shown = live || items[0];
        if (shown) {
            cell.state = !live ? 'withdrawn' : live.confidentiality === 'internal' ? 'internal' : 'published';
            cell.level = shown.integrity_level;
            cell.id = shown.id;
        } else if (quarter === current) {
            cell.state = 'current';
            cell.daysLeft = Math.ceil((Date.UTC(year, ((quarter % 4) * 3) + 3, 1) - nowMs) / DAY_MS);
        }
        return cell;
    }

    /**
     * Quarters at a glance, from the first one a disclosure closes to the current one, twelve
     * at most. Each is published, internal, withdrawn, missing, or current with the days left
     * in it. A disclosure is filed under the quarter its period ends in, and a live copy wins
     * over a withdrawn one of the same quarter.
     * @param {Array<{id: string, period: {to: string}, confidentiality: string, integrity_level: string | null, withdrawn: object | null}>} disclosures
     * @param {number} nowMs
     */
    function periodRail(disclosures, nowMs) {
        if (!disclosures || disclosures.length === 0) return [];
        const now = new Date(nowMs);
        const current = quarterOf(now.getUTCFullYear(), now.getUTCMonth() + 1);
        const byQuarter = new Map();
        disclosures.forEach(function (item) {
            const parts = String(item.period.to).split('-');
            const quarter = quarterOf(Number(parts[0]), Number(parts[1]));
            byQuarter.set(quarter, (byQuarter.get(quarter) || []).concat([item]));
        });
        const quarters = Array.from(byQuarter.keys());
        const end = Math.max(current, Math.max.apply(null, quarters));
        const start = Math.max(Math.min.apply(null, quarters), end - RAIL_QUARTERS + 1);
        const rail = [];
        for (let quarter = start; quarter <= end; quarter++)
            rail.push(railCell(quarter, byQuarter.get(quarter) || [], current, nowMs));
        return rail;
    }

    /**
     * A month's name, `October` in English and `octobre` in French.
     * @param {number} month 1 to 12
     * @param {string} [language] the shown language when omitted
     * @returns {string}
     */
    function monthName(month, language = LANG) {
        if (language !== 'fr') return CARD_MONTHS[month - 1];
        return new Intl.DateTimeFormat('fr-FR', {month: 'long', timeZone: 'UTC'}).format(new Date(Date.UTC(2000, month - 1, 1)));
    }

    /**
     * A disclosure's period in words: `2026 Q3`, `October 2026`, `2026`, or its two days.
     * @param {{type: string, from: string, to: string}} period
     * @returns {string}
     */
    function periodLabel(period) {
        const parts = String(period.to).split('-');
        const month = Number(parts[1]);
        if (period.type === 'calendar-quarter') return t('period.quarter', {year: parts[0], quarter: Math.floor((month - 1) / 3) + 1});
        if (period.type === 'calendar-year') return parts[0];
        if (period.type === 'calendar-month') return monthName(month) + ' ' + parts[0];
        return t('period.range', {from: day(period.from, {withYear: true}), to: day(period.to, {withYear: true})});
    }

    /** Runtime coverage as a whole percent, and whether it falls under the official 75 %. */
    function coverageNotch(coverage) {
        if (coverage == null) return null;
        return {percent: Math.round(coverage * 100), below: coverage < OFFICIAL_COVERAGE};
    }

    /**
     * Who checked what. The Hub checks the content hash and the attestation's binding with
     * its engine. The signature and the audit are only ever declared: whoever runs
     * verify-hash checks the first, nobody here checks the second.
     */
    function checkRows(detail) {
        const checks = detail.checks;
        const audited = detail.disclosure.integrity_level === 'audited';
        const binding = checks.binding === 'no_attestation' ? 'absent' : checks.binding;
        const notHere = t('disc.check.by.not_here');
        const bindingBy = binding === 'checked' ? t('disc.check.by.same_run')
            : binding === 'absent' ? t('disc.check.by.no_attestation') : notHere;
        return [
            {
                claim: t('disc.check.hash'),
                by: checks.content_hash === 'checked' ? t('disc.check.by.hub', {version: checks.engine_version}) : notHere,
                outcome: checks.content_hash
            },
            {
                claim: t('disc.check.binding'),
                by: bindingBy,
                outcome: binding
            },
            {
                claim: t('disc.check.signature'),
                by: t('disc.check.by.you'),
                outcome: checks.signature === 'declared' ? 'declared' : 'absent'
            },
            {
                claim: t('disc.check.audit'),
                by: audited ? t('disc.check.by.report') : t('disc.check.by.nobody'),
                outcome: audited ? 'declared' : 'none'
            }
        ];
    }

    /**
     * The database and broker hosts a disclosure counts inside its total energy, which no
     * application carries: the rows that make the applications table add up to the total.
     * A block with no in-total window gives none, and so does a report older than v1.8.
     */
    function hostRows(report) {
        const aggregate = (report && report.aggregate) || {};
        const breakdown = aggregate.carbon_breakdown || {};
        return [
            [t('disc.host.database'), aggregate.database_waste, breakdown.database_kgco2eq],
            [t('disc.host.broker'), aggregate.messaging_waste, breakdown.messaging_kgco2eq]
        ].filter(function (row) {
            return row[1] && Number(row[1].in_totals_windows) > 0;
        }).map(function (row) {
            const inTotals = Number(row[1].in_totals_windows);
            const windows = Number(row[1].windows_with_figure);
            return {
                label: row[0],
                energy_kwh: Number(row[1].in_totals_energy_kwh) || 0,
                carbon_kgco2eq: Number(row[2]) || 0,
                // The figure was inside the totals for part of the period only.
                partial: windows > inTotals ? {windows: inTotals, of: windows} : null
            };
        });
    }

    /** Host kWh with two decimals, scientific below 0.005 so a small in-total host never reads as zero. */
    function hostKwh(value) {
        return decimal(value === 0 || value >= 0.005 ? value.toFixed(2) : value.toExponential(1));
    }

    /** verify-hash on the downloaded PDF, with this Hub's expected signer when it has one. */
    function verifyCommand(detail) {
        const lines = ['perf-sentinel verify-hash --report report.pdf'];
        if (detail.expected_identity && detail.expected_issuer) {
            lines.push('  --expected-identity ' + shq(detail.expected_identity));
            lines.push('  --expected-issuer ' + shq(detail.expected_issuer));
        }
        return lines.join(' \\\n');
    }

    /** The last steps of the CI job that produces, signs and publishes a disclosure. */
    function publishCommand(origin) {
        return [
            'perf-sentinel disclose --intent official --confidentiality public \\',
            '  --period-type calendar-quarter --from 2026-07-01 --to 2026-09-30 \\',
            '  --input /var/lib/perf-sentinel/archive --org-config org.toml \\',
            '  --output report.json --emit-attestation attestation.intoto.jsonl',
            'cosign sign-blob --bundle bundle.sig --new-bundle-format attestation.intoto.jsonl',
            t('disc.publish.patch'),
            'curl --fail -H "X-API-Key: $HUB_DISCLOSURES_KEY" \\',
            '  -F report=@report.json \\',
            '  -F attestation=@attestation.intoto.jsonl \\',
            '  -F bundle=@bundle.sig \\',
            '  ' + origin + '/api/import/disclosures'
        ].join('\n');
    }

    function saveBlob(blob, filename) {
        const url = URL.createObjectURL(blob);
        const link = document.createElement('a');
        link.href = url;
        link.download = filename;
        link.click();
        setTimeout(function () {
            URL.revokeObjectURL(url);
        }, 4000);
    }

    /**
     * A file saved through fetch and a Blob rather than a plain link, so a lapsed session
     * signs back in instead of saving the 401's body under the file's name.
     * @returns {Promise<boolean>} whether the file was handed to the browser
     */
    function downloadAs(url, filename, env) {
        const io = env || {
            fetch: function (target) {
                return fetch(target, {credentials: 'same-origin'});
            },
            reload: function () {
                location.reload();
            },
            save: saveBlob
        };
        return io.fetch(url).then(function (response) {
            if (response.status === 401) {
                io.reload();
                return false;
            }
            if (!response.ok) return false;
            return response.blob().then(function (blob) {
                io.save(blob, filename);
                return true;
            });
        }, function () {
            return false;
        });
    }

    /**
     * The screen a hash names. `#/access` is one only on a Hub that answered
     * /api/access: with access off, or that read failed, it reads as New, as any
     * unknown route always has. `#/coverage` is an administrator's, and a member who
     * follows a shared link to it lands on their own access instead.
     * @param {string | null | undefined} hash
     * @param {boolean} accessOn
     * @param {boolean} admin
     * @returns {string}
     */
    function screenOf(hash, accessOn, admin) {
        // The query a handoff link carries is the form's, not the route's.
        const route = (hash || '#/new').replace('#/', '').split('?')[0];
        if (route.indexOf('run/') === 0) return 'run';
        if (route.indexOf('report/') === 0) return 'report';
        if (route === 'coverage' && accessOn) return admin ? 'coverage' : 'access';
        if (route === 'access' || route === 'coverage') return accessOn ? 'access' : 'missing';
        if (route === '') return 'new';
        if (route.indexOf('disclosures/') === 0) return 'disclosure';
        // `ack` has no tab: it is reached by link only.
        return ['new', 'recent', 'sources', 'incidents', 'correlations', 'recap', 'disclosures', 'ack'].indexOf(route) >= 0
            ? route
            : 'missing';
    }

    /**
     * The form fields a past run's request fills back in, so running it again
     * starts from what it asked rather than from a blank form.
     * @param {Record<string, unknown> | null | undefined} request
     * @returns {Record<string, unknown>}
     */
    function formFromRequest(request) {
        const r = request || {};
        const form = {detection: Object.assign({}, r['detection'] || {}), pickerOpen: false};
        if (r['trace_id']) return Object.assign(form, {mode: 'trace', traceId: String(r['trace_id'])});
        Object.assign(form, {mode: 'service', service: String(r['service'] || ''), traceId: ''});
        if (typeof r['max_traces'] === 'number') form.maxTraces = r['max_traces'];
        if (typeof r['from_ms'] === 'number' && typeof r['to_ms'] === 'number') {
            return Object.assign(form, {rangeMode: 'absolute', fromMs: r['from_ms'], toMs: r['to_ms']});
        }
        if (r['lookback']) Object.assign(form, {rangeMode: 'relative', lookback: String(r['lookback'])});
        return form;
    }

    /**
     * A machine token's expiry, in whole days left.
     * @param {number} expiresAtMs
     * @param {number} nowMs
     * @returns {string}
     */
    function accessExpiry(expiresAtMs, nowMs) {
        if (expiresAtMs <= nowMs) return t('access.expiry.expired');
        const days = Math.floor((expiresAtMs - nowMs) / 86400000);
        if (days === 0) return t('access.expiry.within_day');
        return t('access.expiry.days.' + plural(days), {count: days});
    }

    const ACCESS_CALLER = textTable('access.caller.', ['session', 'terminal_token', 'machine_token']);

    /**
     * The access page's first block. A session ends within the day, so its time is
     * enough; a token's date is not.
     * @param {{identity: string | null, subject: string, caller: string, expires_at_ms: number | null}} access
     * @param {number} nowMs
     * @returns {{line: string, note: string | null, subjectLabel: string, subject: string}}
     */
    function accessSignedIn(access, nowMs) {
        const expires = access.expires_at_ms;
        const until = expires == null ? ''
            : t('access.until', {when: expires - nowMs < 86400000 ? time(expires) : dateTime(expires)});
        return {
            line: t('access.signed_in', {
                who: access.identity || access.subject,
                via: ACCESS_CALLER[/** @type {keyof typeof ACCESS_CALLER} */ (access.caller)] || access.caller,
                until: until
            }),
            // A machine token's group is configuration, read on every request.
            note: access.caller === 'machine_token' ? null : t('access.group_note'),
            subjectLabel: t('access.subject_label'),
            subject: access.subject
        };
    }

    /**
     * The "What you reach" table. A grant on `*` covers more than every project, and
     * an entry with no group is Admins, which names the caller's id.
     * @param {{reach: {group: string | null, project: string, role: string, permissions: string[]}[]}} access
     * @returns {{project: string, group: string, role: string, permissions: string}[]}
     */
    function accessReachRows(access) {
        return access.reach.map(entry => ({
            project: entry.project === '*' ? t('access.reach.everything') : entry.project,
            group: entry.group == null ? t('access.reach.admins') : entry.group,
            role: entry.role,
            permissions: entry.permissions.join(', ')
        }));
    }

    /**
     * The patterns that would cover what no project covers, one YAML flow mapping per line,
     * ready to paste under a project's patterns. Values are JSON-quoted, which YAML reads as
     * double-quoted strings, so a `*` or a `=` cannot change their meaning. Null when nothing
     * is left uncovered. A finding with no service has no service pattern to offer.
     * @param {{services: {service: string | null}[], attributes: {key: string, value: string}[],
     *          sources: {source_id: string}[]}} unmatched
     * @returns {string | null}
     */
    function coverageSnippet(unmatched) {
        const lines = []
            .concat(unmatched.services.filter(row => row.service != null)
                .map(row => '- { service: ' + JSON.stringify(row.service) + ' }'))
            .concat(unmatched.attributes.map(row => '- { attribute: ' + JSON.stringify(row.key + '=' + row.value) + ' }'))
            .concat(unmatched.sources.map(row => '- { source: ' + JSON.stringify(row.source_id) + ' }'));
        if (lines.length === 0) return null;
        return [t('coverage.snippet.line1'), t('coverage.snippet.line2')].concat(lines).join('\n');
    }

    /**
     * What a caller no grant reaches sends an administrator: the name, the id an
     * administrator lists, and the groups a grant can name, with where each comes from.
     * @param {{identity: string | null, subject: string, groups: {name: string, origin: string}[]}} access
     * @returns {string}
     */
    function accessRequestText(access) {
        const groups = access.groups.length === 0
            ? t('access.request.none')
            : access.groups.map(group => group.name + ' (' + group.origin + ')').join(', ');
        return t('access.request.text', {name: access.identity || access.subject, id: access.subject, groups: groups});
    }

    /**
     * The source panel's count. With access on a source the caller may not run on is left
     * out of the list, and the count says so rather than quietly shrinking.
     * @param {number} runnable
     * @param {number} total
     * @returns {string}
     */
    function sourceCountLabel(runnable, total) {
        if (runnable === total) return t('new.sources.count', {total: total});
        const missing = total - runnable;
        return t(missing === 1 ? 'new.sources.count_missing.one' : 'new.sources.count_missing.many',
            {runnable: runnable, total: total, missing: missing});
    }

    /**
     * What the tokens card says after a Revoke. A 404 is a token already gone. Status 0
     * stands for no answer at all.
     * @param {number} status
     * @param {{owner: string, created: string}} row
     * @returns {{ok: boolean, text: string}}
     */
    function revokeOutcome(status, row) {
        const which = {owner: row.owner, created: row.created, status: status};
        if (status === 204) return {ok: true, text: t('access.revoke.done', which)};
        if (status === 404) return {ok: true, text: t('access.revoke.gone', which)};
        return {ok: false, text: t(status === 0 ? 'access.revoke.no_answer' : 'access.revoke.failed', which)};
    }

    /**
     * The access page's terminal token table, one row per token: who owns it, where it was
     * approved for, and when it was made, expires and was last used (to five minutes).
     * @param {{id: string, identity: string, client_address: string | null, created_at_ms: number,
     *          expires_at_ms: number, last_used_at_ms: number | null}[]} tokens
     * @returns {{id: string, owner: string, from: string, created: string, expires: string, lastUsed: string}[]}
     */
    function tokenRows(tokens) {
        return tokens.map(token => ({
            id: token.id,
            owner: token.identity,
            from: token.client_address || t('access.token.unknown'),
            created: dateTime(token.created_at_ms),
            expires: dateTime(token.expires_at_ms),
            lastUsed: token.last_used_at_ms == null ? t('access.token.never') : dateTime(token.last_used_at_ms)
        }));
    }

    /**
     * The revoke request. JSON even with no body: the Hub refuses any other write from a page.
     * @param {string} id
     * @returns {{path: string, init: {method: string, headers: Record<string, string>}}}
     */
    function tokenRevokeRequest(id) {
        return {
            path: '/api/tokens/' + encodeURIComponent(id),
            init: {method: 'DELETE', headers: {'content-type': 'application/json'}}
        };
    }

    /**
     * The approval page's first line, with the session's end once known.
     * @param {number | null} expiresAtMs
     * @returns {string}
     */
    function deviceIntro(expiresAtMs) {
        return t('device.intro', {until: expiresAtMs == null ? t('device.intro.session_end') : time(expiresAtMs)});
    }

    /**
     * Where and when a code was requested, who approves it and until when the token holds.
     * @param {{client_address: string, requested_at_ms: number, identity: string | null,
     *          token_expires_at_ms: number | null}} verify
     * @param {number} nowMs
     * @returns {string}
     */
    function deviceRequestLine(verify, nowMs) {
        const minutes = Math.floor((nowMs - verify.requested_at_ms) / 60000);
        return t('device.request', {
            address: verify.client_address,
            at: clock(verify.requested_at_ms).slice(0, 8),
            ago: minutes < 1 ? t('device.ago.less') : t('device.ago.' + plural(minutes), {count: minutes}),
            who: verify.identity || t('device.identity.unknown'),
            expires: verify.token_expires_at_ms == null ? t('device.expires.session_end') : time(verify.token_expires_at_ms)
        });
    }

    /**
     * The warning shown above the buttons when the code came from another network than this
     * browser, the one cheap signal that tells a remote phish from a login on this machine.
     * @param {{client_address: string, same_network: boolean}} verify
     * @returns {string | null}
     */
    function deviceNetworkAlert(verify) {
        return verify.same_network ? null : t('device.network_alert', {address: verify.client_address});
    }

    /**
     * What a click on Approve does: send, or first turn into "Approve anyway" when the code
     * came from another network.
     * @param {boolean} sameNetwork
     * @param {boolean} armed
     * @returns {'send' | 'arm'}
     */
    function deviceApproveStep(sameNetwork, armed) {
        return sameNetwork || armed ? 'send' : 'arm';
    }

    /**
     * An action owner as a person reads it. The name the SSO gave wins, with the full identity
     * on hover. A machine owner or one typed by hand (a space or an upper-case letter) shows
     * as it is, and an identity (`alice.martin@acme.com`, `local:alice`) is turned into a name.
     * @param {string | null | undefined} owner
     * @param {string | null | undefined} ownerDisplay
     * @returns {{text: string, title: string | null}}
     */
    function ownerLabel(owner, ownerDisplay) {
        if (ownerDisplay) return {text: ownerDisplay, title: owner || null};
        if (!owner) return {text: '', title: null};
        if (owner.startsWith('machine:') || /\s|\p{Lu}/u.test(owner)) return {text: owner, title: null};
        const upper = (word) => {
            const first = String.fromCodePoint(word.codePointAt(0));
            return first.toLocaleUpperCase() + word.slice(first.length);
        };
        const name = owner.replace(/^local:/, '').split('@')[0].split(/[._]/).filter(Boolean)
            .map((word) => word.split('-').filter(Boolean).map(upper).join('-')).join(' ');
        return name ? {text: name, title: owner} : {text: owner, title: null};
    }

    // ----------------------------------------------------------- the recap screen

    /**
     * Labels that differ by audience. Management reads none of the engine's words: the same
     * figure is "Unnecessary operations" there and "Avoidable I/O" for the engineers. The
     * carbon and action tiles keep one label.
     */
    const RECAP_LABELS = {
        tech: textTable('recap.label.tech.', ['io', 'findings', 'moved']),
        pilotage: textTable('recap.label.pilotage.', ['io', 'findings', 'moved'])
    };

    /**
     * The text behind the seven "?" of the recap, one set per audience. It is copy, pinned by a
     * test: the Management set may not use the engine's words, and the frozen report (no
     * script, so no "?") prints the same sentences in its closing section.
     */
    const RECAP_HELP_KEYS = ['period', 'io', 'co2', 'findings', 'actions', 'prio', 'moved'];
    const RECAP_HELP = {
        tech: textTable('recap.help.tech.', RECAP_HELP_KEYS),
        pilotage: textTable('recap.help.pilotage.', RECAP_HELP_KEYS)
    };

    const RECAP_PERIODS = ['day', 'week', 'month'];

    /**
     * A service as a row shows it: by the business name a team gave it, with the technical name
     * beside it once named (Engineering), the name alone where a Management reader sees it.
     * @param {string} service
     * @param {string | null | undefined} serviceLabel
     * @returns {{name: string, code: string | null}}
     */
    function serviceParts(service, serviceLabel) {
        return serviceLabel ? {name: serviceLabel, code: service} : {name: service, code: null};
    }

    /**
     * Whether the card at `index` is the first of its service in `rows`: only that one offers to
     * name the service, so a list of one service's steps does not repeat the button.
     * @param {Array<{service: string}>} rows
     * @param {number} index
     * @returns {boolean}
     */
    function namesService(rows, index) {
        return rows.findIndex(function (row) {
            return row.service === rows[index].service;
        }) === index;
    }

    /**
     * What a `#/recap?…` hash names. Anything it does not know reads as the default, and a filter
     * that is blank, past 256 characters or carries a control character as none.
     * @param {string | null | undefined} hash
     * @returns {{period: 'day' | 'week' | 'month', at: string | null, environment: string | null,
     *   service: string | null, grouping: string | null, view: 'tech' | 'pilotage'}}
     */
    function readRecapRoute(hash) {
        const text = String(hash || '');
        const query = text.indexOf('#/recap?') === 0 ? text.slice('#/recap?'.length) : '';
        const params = new URLSearchParams(query);
        const filter = function (key) {
            const value = (params.get(key) || '').trim();
            return value === '' || value.length > 256 || /\p{Cc}/u.test(value) ? null : value;
        };
        const period = params.get('period') || '';
        const at = params.get('at') || '';
        return {
            period: RECAP_PERIODS.indexOf(period) >= 0 ? /** @type {'day' | 'week' | 'month'} */ (period) : 'week',
            at: /^\d{4}-\d{2}-\d{2}$/.test(at) ? at : null,
            environment: filter('environment'),
            service: filter('service'),
            grouping: filter('grouping'),
            view: params.get('view') === 'pilotage' ? 'pilotage' : 'tech'
        };
    }

    /**
     * The route of a recap read: `#/recap` and only the values that differ from the defaults.
     * @param {ReturnType<typeof readRecapRoute>} route
     * @returns {string}
     */
    function recapHash(route) {
        const parts = [];
        if (route.period !== 'week') parts.push('period=' + encodeURIComponent(route.period));
        ['at', 'environment', 'service', 'grouping'].forEach(function (key) {
            if (route[key]) parts.push(key + '=' + encodeURIComponent(route[key]));
        });
        if (route.view !== 'tech') parts.push('view=' + encodeURIComponent(route.view));
        return parts.length ? '#/recap?' + parts.join('&') : '#/recap';
    }

    /**
     * The API read for a route: the period and the view always, the filters when set.
     * @param {ReturnType<typeof readRecapRoute>} route
     * @returns {string}
     */
    function recapPath(route) {
        let path = '/api/recap?period=' + encodeURIComponent(route.period)
            + '&view=' + encodeURIComponent(route.view);
        ['at', 'environment', 'service', 'grouping'].forEach(function (key) {
            if (route[key]) path += '&' + key + '=' + encodeURIComponent(route[key]);
        });
        return path;
    }

    /**
     * The line that names a frozen report: its period as the recap prints it, then the
     * subscription that made it or the person who asked, and a mark when it is not whole.
     * @param {{subscription: string | null, created_by?: string | null, complete: boolean,
     *   period: {kind: string, from: string, to: string}}} report
     * @returns {string}
     */
    function recapReportLabel(report) {
        const kind = report.period.kind;
        const period = periodText(report.period);
        const head = hasText('recap.report.kind.' + kind)
            ? t('recap.report.kind.' + kind, {period: period})
            : kind.charAt(0).toUpperCase() + kind.slice(1) + ' ' + period;
        const origin = report.subscription
            || t('recap.report.on_demand', {who: report.created_by || t('recap.report.someone')});
        return head + ' · ' + origin + (report.complete ? '' : t('recap.report.incomplete'));
    }

    /** The fixed copy of the Reports section, pinned with the Management words test. */
    const RECAP_REPORTS_COPY = textTable('recap.reports.', ['title', 'hint', 'help', 'generate', 'generating', 'none',
        'failed', 'refused']);

    /**
     * What Generate a report would freeze for a route.
     * @param {ReturnType<typeof readRecapRoute>} route
     * @returns {string}
     */
    function recapFreezeText(route) {
        const filters = route.environment || route.service || route.grouping ? t('recap.freeze.filters') : '';
        return route.at
            ? t('recap.freeze.at.' + route.period, {at: recapDay(route.at), filters: filters})
            : t('recap.freeze.last.' + route.period, {filters: filters});
    }

    /**
     * Why the recap did not load. Management gets no permission name and no daemon.
     * @param {number} status
     * @param {string} view
     * @returns {string}
     */
    function recapErrorText(status, view) {
        const audience = view === 'pilotage' ? 'pilotage' : 'tech';
        if (status === 403) return t('recap.error.forbidden.' + audience);
        if (status === 400) return t('recap.error.invalid');
        return t('recap.error.other.' + audience);
    }

    /**
     * The note under a ranking the Hub cut at 5,000, as RecapText.Truncated.
     * @param {string} view
     * @returns {string}
     */
    function recapTruncatedText(view) {
        return t('recap.truncated.' + (view === 'pilotage' ? 'pilotage' : 'tech'));
    }

    /**
     * The sources that sent no detection volume, as RecapText.SourcesWithoutVolume.
     * @param {string[]} sources
     * @param {string} view
     * @returns {string}
     */
    function recapSourcesNote(sources, view) {
        return t('recap.sources_note.' + (view === 'pilotage' ? 'pilotage' : 'tech'), {sources: sources.join(', ')});
    }

    /**
     * The sources left out of the period, as RecapText.LateSources: the same words in both views.
     * @param {string[]} sources
     * @returns {string}
     */
    function recapLateNote(sources) {
        return t('recap.late_note', {sources: sources.join(', ')});
    }

    /**
     * Why a report went nowhere, or an empty string when it went somewhere.
     * @param {{subscription: string | null, nothing_moved: boolean, deliveries: unknown[]}} report
     * @returns {string}
     */
    function recapReportStatus(report) {
        if (!report.subscription) return t('recap.report.status.on_demand');
        if (report.nothing_moved) return t('recap.report.status.nothing_moved');
        return report.deliveries.length === 0 ? t('recap.report.status.no_destination') : '';
    }

    /**
     * One delivery of a report: the channel and where it went, the state, the last answer the
     * destination gave and how many tries it took.
     * @param {{channel: string, target: string, state: string, attempts?: number,
     *   last_status?: number | null}} delivery
     * @returns {string}
     */
    function recapDeliveryText(delivery) {
        const attempts = delivery.attempts || 0;
        const state = hasText('recap.delivery.state.' + delivery.state)
            ? t('recap.delivery.state.' + delivery.state) : delivery.state;
        return t('recap.delivery.line', {channel: delivery.channel, target: delivery.target, state: state})
            + (delivery.last_status === null || delivery.last_status === undefined ? '' : ' (' + delivery.last_status + ')')
            + (attempts > 0 ? t('recap.delivery.attempts.' + plural(attempts), {count: attempts}) : '');
    }

    /**
     * The body of a generate request: the screen's period and filters, and the date only when
     * the route names one (the Hub then takes the last closed period).
     * @param {ReturnType<typeof readRecapRoute>} route
     * @returns {{period: string, at?: string, environment: string | null, service: string | null,
     *   grouping: string | null}}
     */
    function recapOnDemandBody(route) {
        return {
            period: route.period,
            ...(route.at ? {at: route.at} : {}),
            environment: route.environment,
            service: route.service,
            grouping: route.grouping
        };
    }

    /**
     * The carbon tile in three pieces, so a phone-wide tile can set the figure large and the
     * rest small. The grams arrive rounded to one significant figure, the server's rule.
     * @param {number | null} grams
     * @param {number} sourcesWithoutCoefficient
     * @param {string | null | undefined} missingReason `no_coefficient` or `no_avoidable_io`
     * @param {string} [view] `pilotage` says only that there is no figure
     * @returns {{lead: string, figure: string, unit: string}}
     */
    function co2Parts(grams, sourcesWithoutCoefficient, missingReason, view) {
        if (grams === null || grams === undefined) {
            if (view === 'pilotage') return {lead: '', figure: '—', unit: t('recap.co2.na')};
            return {
                lead: '', figure: '—',
                unit: t(missingReason === 'no_avoidable_io' ? 'recap.co2.no_avoidable_io' : 'recap.co2.no_coefficient')
            };
        }
        // A nonzero estimate never rounds to zero; "at least" would contradict "< 1 g".
        if (grams > 0 && grams < 1) return {lead: '', figure: '< 1 g', unit: 'CO2e'};
        const kilos = grams >= 1000;
        const value = kilos ? Number((grams / 1000).toPrecision(3)) : Math.round(grams);
        return {
            lead: sourcesWithoutCoefficient > 0 ? t('recap.co2.lead') : '',
            figure: '~' + num(value) + (kilos ? ' kg' : ' g'),
            unit: 'CO2e'
        };
    }

    /**
     * The carbon estimate as one sentence: an estimate always, a partial minimum when some
     * sources send no coefficient, a dash and the reason when there is no figure.
     * @param {number | null} grams
     * @param {number} sourcesWithoutCoefficient
     * @param {string | null | undefined} missingReason
     * @param {string} [view]
     * @returns {string}
     */
    function co2Text(grams, sourcesWithoutCoefficient, missingReason, view) {
        const parts = co2Parts(grams, sourcesWithoutCoefficient, missingReason, view);
        if (grams === null || grams === undefined) return parts.figure + ' ' + parts.unit;
        return t('recap.co2.text', {
            value: (parts.lead ? parts.lead + ' ' : '') + parts.figure + ' ' + parts.unit,
            tail: co2Tail(sourcesWithoutCoefficient, view)
        });
    }

    /**
     * What a partial carbon figure leaves out, after a comma, or nothing when it is whole.
     * @param {number} sourcesWithoutCoefficient
     * @param {string} [view]
     * @returns {string}
     */
    function co2Tail(sourcesWithoutCoefficient, view) {
        if (!(sourcesWithoutCoefficient > 0)) return '';
        return view === 'pilotage'
            ? t('recap.co2.tail.pilotage')
            : t('recap.co2.tail.tech.' + plural(sourcesWithoutCoefficient), {count: sourcesWithoutCoefficient});
    }

    /**
     * Whether a ranked item carries a volume to read. The Hub ranks on a volume above zero, so a
     * zero is as much "nothing to count" as null and must never show as a figure.
     * @param {{avoidable_io_ops?: number | null}} item
     * @returns {boolean}
     */
    function hasAvoidableVolume(item) {
        return item.avoidable_io_ops != null && item.avoidable_io_ops > 0;
    }

    /**
     * Whether the "Below, ..." divider goes before a ranked row: the first row without a volume
     * after one with, so a list with no volume at all has none.
     * @param {Array<{avoidable_io_ops?: number | null}>} items the ranking, volumes first
     * @param {number} index
     * @returns {boolean}
     */
    function dividesBefore(items, index) {
        return index > 0 && !hasAvoidableVolume(items[index]) && hasAvoidableVolume(items[index - 1]);
    }

    /**
     * A row's trend as a person reads it: a word, or the relative change as a signed percentage.
     * @param {string} trend `new`, `up`, `down`, `flat`, `gone` or `unknown`
     * @param {number | null | undefined} change
     * @returns {string}
     */
    function trendText(trend, change) {
        if (['new', 'gone', 'flat', 'up', 'down'].indexOf(trend) < 0) return '';
        if (trend !== 'up' && trend !== 'down' || change === null || change === undefined) return t('recap.trend.' + trend);
        // The narrow space by hand, as pct writes it.
        return (change > 0 ? '+' : '') + Math.round(change * 100) + (LANG === 'fr' ? '\u202f%' : '%');
    }

    /**
     * What the engine's check of a `done` action says, in words. A state a newer Hub adds is
     * left out rather than guessed at.
     * @param {string | null | undefined} state
     * @param {string} [view] `pilotage` for the Management words
     * @returns {string}
     */
    function confirmationLabel(state, view) {
        switch (state) {
            case 'confirmed':
            case 'awaiting':
                return t('recap.confirm.' + state + (view === 'pilotage' ? '.pilotage' : ''));
            case 'contradicted':
            case 'came_back':
                return t('recap.confirm.' + state);
            default:
                return '';
        }
    }

    /**
     * The days of a period, UTC as the server counts them, and whether it is still running.
     * @param {{kind: string, from: string, to: string, in_progress?: boolean}} period
     * @returns {string}
     */
    function periodText(period) {
        const days = period.kind === 'day'
            ? recapDay(period.from) : t('recap.period.range', {from: recapDay(period.from), to: recapDay(period.to)});
        return period.in_progress ? t('recap.period.in_progress', {days: days}) : days;
    }

    /**
     * A recap day: `yyyy-MM-dd` as the server counts it in English, `29 oct. 2026` in French,
     * as RecapText.Day.
     * @param {string} iso
     * @returns {string}
     */
    function recapDay(iso) {
        return LANG === 'fr' ? day(iso, {withYear: true, short: true}) : iso;
    }

    /**
     * A Management row's plain sentence, read now in the shown language: a frozen snapshot holds it in
     * English, and a type this launcher does not know keeps the sentence it came with.
     * @param {{finding_type: string, plain: string}} item
     * @returns {string}
     */
    function recapPlain(item) {
        return hasText('recap.plain.' + item.finding_type) ? t('recap.plain.' + item.finding_type) : item.plain;
    }

    /**
     * @param {string[] | null | undefined} reasons
     * @param {string} [view]
     * @returns {string}
     */
    function incompleteText(reasons, view) {
        return reasons && reasons.length ? t('recap.incomplete', {
            reasons: reasons.map(function (reason) {
                return gapLabel(reason, view);
            }).join(', ')
        }) : '';
    }

    /**
     * @param {string[] | null | undefined} notes
     * @param {number | null | undefined} samplingRate
     * @param {string} [view]
     * @returns {string}
     */
    function notesText(notes, samplingRate, view) {
        if (!notes || !notes.length) return '';
        const hasRate = samplingRate !== null && samplingRate !== undefined;
        return t('recap.notes', {
            notes: notes.map(function (note) {
                return gapLabel(note, view)
                    + (note === 'sampling' && hasRate ? t('recap.notes.rate', {rate: decimal(samplingRate)}) : '');
            }).join(', ')
        });
    }

    /**
     * The address when it is an http or https one, else null. A `reference_url` comes from a daemon,
     * so a `javascript:` address must never reach an href.
     * @param {string | null | undefined} value
     * @returns {string | null}
     */
    function httpUrl(value) {
        try {
            const url = new URL(String(value));
            return url.protocol === 'http:' || url.protocol === 'https:' ? String(value) : null;
        } catch (error) {
            return null;
        }
    }

    /**
     * A timestamp in the reader's own time and zone, always `YYYY-MM-DD HH:mm <zone>`: the locale
     * only supplies the zone name (en-GB writes CET where en-US writes GMT+1), the order is fixed.
     * @param {number} ms
     * @returns {string}
     */
    function localTime(ms) {
        const parts = {};
        new Intl.DateTimeFormat('en-GB', {
            year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit',
            hour12: false, timeZoneName: 'short'
        }).formatToParts(ms).forEach(function (part) {
            parts[part.type] = part.value;
        });
        // Some engines write midnight as 24.
        const hour = parts.hour === '24' ? '00' : parts.hour;
        const date = LANG === 'fr'
            ? parts.day + '/' + parts.month + '/' + parts.year : parts.year + '-' + parts.month + '-' + parts.day;
        return date + ' ' + hour + ':' + parts.minute + ' ' + parts.timeZoneName;
    }

    // ------------------------------------------------------------ share cards

    /** The kinds a card covers, in the order the toggle shows them. */
    const CARD_PERIODS = ['week', 'month', 'quarter', 'semester', 'year'];
    /** "Semester" reads as a school term: the screen says Half-year, the value sent stays semester. */
    const CARD_PERIOD_LABELS = textTable('card.period.', CARD_PERIODS);
    const CARD_ANGLES = ['story', 'curve', 'collective'];
    const CARD_ANGLE_NAMES = textTable('card.angle.', CARD_ANGLES);
    const CARD_ANGLE_LINES = textTable('card.angle_line.', CARD_ANGLES);
    const CARD_MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September',
        'October', 'November', 'December'];
    const CARD_KIND_MONTHS = {month: 1, quarter: 3, semester: 6, year: 12};
    /** The files a card downloads as; the bracketed type is the one the saved file carries. */
    const CARD_FORMATS = ['png', 'svg', 'jxl', 'pdf'];
    const CARD_FORMAT_LABELS = textTable('card.format.', CARD_FORMATS);
    /** The sentence under a format while it is picked, where the choice alone does not say where the file goes. */
    const CARD_FORMAT_NOTES = textTable('card.format_note.', ['jxl']);

    /** The format a preview renders: a JPEG XL previews as the PNG of the same card, which every browser shows. */
    function previewFormat(format) {
        return format === 'jxl' ? 'png' : format;
    }

    /**
     * The wait a busy render gate asks for, or null: its 503 carries Retry-After, the engine-off 503 does not.
     * Whole seconds from 1 to 10; a date form, or anything else, reads as 1.
     */
    function cardBusy(status, retryAfter) {
        if (status !== 503 || retryAfter === null || retryAfter === undefined) return null;
        const seconds = /^\d+$/.test(retryAfter.trim()) ? Number(retryAfter.trim()) : 1;
        return {text: CARD_COPY.busy, delayMs: Math.min(10, Math.max(1, seconds)) * 1000};
    }

    /** Every other sentence of the card section, in one place for the copy review. */
    const CARD_COPY = textTable('card.copy.', ['title', 'hint', 'help', 'closed', 'create', 'intro', 'engineOff',
        'optionsFailed', 'noFix', 'fixesWait', 'defaultPill', 'labelHint', 'rememberDenied', 'freeNote', 'carouselNote',
        'fieldWhy', 'stale', 'previewEmpty', 'previewBlocked', 'rendering', 'busy', 'pngNote', 'svgNote', 'pdfNote',
        'noDomain', 'keysTitle', 'keysOrigin', 'keysNone', 'keysPublish', 'keysFile', 'rotate', 'leaked']);

    /** One plain sentence per verify-card reason the engine answers at level 1. */
    const ATTACH_REASONS = textTable('card.attach.reason.', ['binding_check_failed', 'binding_partial',
        'key_not_published', 'no_domain', 'no_http_client']);

    /** Level 1 answers where the check succeeded: they state the verdict, never "Could not check". */
    const ATTACH_VERDICTS = textTable('card.attach.verdict.', ['key_revoked', 'public_key_mismatch',
        'signed_outside_validity']);

    /** The signed figures verify-card reads back, in the order the screen lists them. */
    const SIGNED_FIGURE_LABELS = textTable('card.figure.', ['period', 'before_fix_period', 'cited_ops', 'cited_ms',
        'before_fix_ops', 'threshold_ops', 'threshold_ms', 'last_waste_permille', 'previous_waste_permille',
        'confirmed_fixes_count', 'services_count']);

    /** @param {string} day yyyy-MM-dd @returns {Date} midnight UTC */
    function cardDate(day) {
        return new Date(day + 'T00:00:00Z');
    }

    /** @param {Date} date @returns {string} yyyy-MM-dd */
    function cardIso(date) {
        return date.toISOString().slice(0, 10);
    }

    /**
     * The first day of the period of `kind` that holds `day`; a week starts on Monday, UTC.
     * @param {string} kind @param {string} day @returns {string}
     */
    function cardPeriodStart(kind, day) {
        const date = cardDate(day);
        if (kind === 'week') {
            date.setUTCDate(date.getUTCDate() - (date.getUTCDay() + 6) % 7);
            return cardIso(date);
        }
        const months = CARD_KIND_MONTHS[kind];
        const month = Math.floor(date.getUTCMonth() / months) * months;
        return cardIso(new Date(Date.UTC(date.getUTCFullYear(), month, 1)));
    }

    /**
     * The first day of the period `delta` steps away from the one starting on `from`.
     * @param {string} kind @param {string} from @param {number} delta @returns {string}
     */
    function stepPeriod(kind, from, delta) {
        const date = cardDate(from);
        if (kind === 'week') date.setUTCDate(date.getUTCDate() + 7 * delta);
        else date.setUTCMonth(date.getUTCMonth() + CARD_KIND_MONTHS[kind] * delta);
        return cardIso(date);
    }

    /**
     * The first day of the last period of `kind` that ended before `today` (UTC).
     * @param {string} kind @param {string} today @returns {string}
     */
    function lastClosedPeriod(kind, today) {
        return stepPeriod(kind, cardPeriodStart(kind, today), -1);
    }

    /** False at the last closed period, so an open one can never be picked. */
    function canStepForward(kind, from, today) {
        return stepPeriod(kind, from, 1) <= lastClosedPeriod(kind, today);
    }

    function cardDayText(day, withYear) {
        const date = cardDate(day);
        return date.getUTCDate() + ' ' + CARD_MONTHS[date.getUTCMonth()] + (withYear ? ' ' + date.getUTCFullYear() : '');
    }

    /**
     * A period in words, as CardRules.PeriodName writes it.
     * @param {string} kind @param {string} from @returns {string}
     */
    function cardPeriodName(kind, from, language = LANG) {
        const date = cardDate(from);
        const year = String(date.getUTCFullYear());
        if (kind === 'week') return tIn(language, 'card.period_name.week', {day: day(from, {withYear: true, language})});
        if (kind === 'month') {
            return tIn(language, 'card.period_name.month', {month: monthName(date.getUTCMonth() + 1, language), year});
        }
        if (kind === 'quarter') {
            return tIn(language, 'card.period_name.quarter', {quarter: Math.floor(date.getUTCMonth() / 3) + 1, year});
        }
        if (kind === 'semester') {
            return tIn(language, date.getUTCMonth() === 0 ? 'card.period_name.first_half' : 'card.period_name.second_half',
                {year});
        }
        return year;
    }

    /**
     * Why no card, from its code and its period, as CardRules.Explain writes it in English: the
     * period before is named for a curve's refusal, every other code reads its problem sentence.
     * @param {string} code @param {string} kind @param {string} from
     * @returns {string}
     */
    function cardRefusalText(code, kind, from) {
        if (!hasText('card.refusal.' + code)) return problemText({code: code});
        return t('card.refusal.' + code, {
            period: cardPeriodName(kind, from), previous: cardPeriodName(kind, stepPeriod(kind, from, -1))
        });
    }

    /** The equivalence countries the screen offers; another country has no table of its own and sends fr. */
    const CARD_COUNTRIES = ['fr', 'gb', 'us', 'other'];

    /**
     * The country choice shown: the one a person picked, else the Hub's, else France.
     * @param {string|null} picked @param {string|undefined} hubCountry @returns {string}
     */
    function cardCountry(picked, hubCountry) {
        if (picked !== null && CARD_COUNTRIES.indexOf(picked) >= 0) return picked;
        return hubCountry !== undefined && CARD_COUNTRIES.indexOf(hubCountry) >= 0 ? hubCountry : 'fr';
    }

    /** The equivalence_country a choice sends. @param {string} choice @returns {string} */
    function cardCountryWire(choice) {
        return choice === 'other' ? 'fr' : choice;
    }

    /** The card's language: the one a person picked, else the Hub's. @param {string|null} picked */
    function cardLang(picked) {
        return LANGS.indexOf(picked) >= 0 ? /** @type {'en' | 'fr'} */ (picked) : LANG;
    }

    /**
     * The days a period covers: `1 to 31 October 2026, UTC`, the shared month or year written once.
     * @param {string} from @param {string} to @returns {string}
     */
    function cardRangeText(from, to) {
        let start = day(from, {withYear: true});
        if (from.slice(0, 7) === to.slice(0, 7)) start = day(from).split(' ')[0];
        else if (from.slice(0, 4) === to.slice(0, 4)) start = day(from);
        return t('card.range', {from: start, to: day(to, {withYear: true})});
    }

    function cardText(value) {
        return typeof value === 'string' ? value.trim() : '';
    }

    /**
     * The body of a render, with only the values set: the fix and its label on a story only.
     * @param {Object<string, string|null|undefined>} form
     * @returns {Object<string, string>}
     */
    function cardBody(form) {
        /** @type {Object<string, string>} */
        const body = {};
        ['angle', 'period', 'at', 'environment', 'source_id', 'signature', 'label', 'title', 'team', 'lang',
            'equivalence_country', 'format']
            .forEach(function (key) {
                const value = cardText(form[key]);
                if (value && ((key !== 'signature' && key !== 'label') || form.angle === 'story')) body[key] = value;
            });
        return body;
    }

    /** The options read of a form: the period always, the day and the scope when set. */
    function cardOptionsPath(form) {
        let path = '/api/cards/options?period=' + encodeURIComponent(form.period);
        ['at', 'environment', 'source_id'].forEach(function (key) {
            const value = cardText(form[key]);
            if (value) path += '&' + key + '=' + encodeURIComponent(value);
        });
        return path;
    }

    /** Whether the caller may name an action anywhere, which remembering a card's label does. */
    function canRememberLabel(sources) {
        return Array.isArray(sources) && sources.some(function (source) {
            return permits(source, 'labels.write');
        });
    }

    /**
     * The remember box: it waits for a new name, or for one that differs from the remembered one.
     * @param {string} typed @param {string|null} remembered @param {string} place
     * @returns {{text: string, enabled: boolean}}
     */
    function rememberState(typed, remembered, place) {
        const text = cardText(typed);
        return remembered
            ? {text: t('card.remember.update'), enabled: text !== '' && text !== remembered}
            : {text: t('card.remember.new', {place: place}), enabled: text !== ''};
    }

    // A heuristic for typing only, the engine's 422 stays the authority. (C) (R) and TM are pictographic
    // to Unicode but drawn by the card's fonts.
    const CARD_UNDRAWABLE = /[\p{Cc}\p{Cf}]|(?![\u00A9\u00AE\u2122])\p{Extended_Pictographic}/u;

    /** The first character a free field holds that the card cannot draw, as a sentence, or null. */
    function cardFieldProblem(text) {
        const found = String(text || '').match(CARD_UNDRAWABLE);
        return found ? t('card.field_problem', {char: found[0]}) : null;
    }

    /**
     * Whether a curve's 422 is the refusal of the period before, which a story or a collective card
     * escapes. Its code starts with previous_; without a code, CardRules.Explain's "No curve for ...".
     * @param {string | null | undefined} detail
     * @param {string | null} [code]
     */
    function isPreviousRefusal(detail, code) {
        if (code) return code.indexOf('previous_') === 0;
        return String(detail || '').indexOf('No curve for ') === 0;
    }

    /** The two hosts the attachment check reaches. */
    function cardContactText(domain, view) {
        return t(view === 'pilotage' ? 'card.contact.pilotage' : 'card.contact.tech', {domain: domain});
    }

    /**
     * verify-card's verdict as one sentence; the engine's raw reason goes in a title only.
     * @param {{integrity: string, content?: string, level: number, domain: string|null, reason: string|null}} result
     * @param {boolean} isAdmin
     * @returns {{text: string, title: string|null}}
     */
    function attachmentText(result, isAdmin) {
        const domain = {domain: result.domain};
        if (result.integrity !== 'ok') return {text: t('card.attach.tampered'), title: null};
        if (result.content === 'modified') return {text: t('card.attach.modified'), title: 'content_modified'};
        if (result.level === 2) return {text: t('card.attach.attached', domain), title: null};
        if (ATTACH_VERDICTS[result.reason]) {
            return {text: ATTACH_VERDICTS[result.reason].replace('{domain}', result.domain), title: result.reason};
        }
        if (result.reason && result.reason !== 'key_not_published') {
            const reason = ATTACH_REASONS[result.reason];
            return {
                text: t('card.attach.unchecked', {
                    reason: reason ? reason.replace('{domain}', result.domain) : t('card.attach.unexpected')
                }),
                title: result.reason
            };
        }
        return {text: t(isAdmin ? 'card.attach.not_yet.admin' : 'card.attach.not_yet.other', domain), title: null};
    }

    /**
     * The sentence under the verdict when the file is the one signed; a modified file already has its own.
     * @param {{content: string}} result @returns {string|null}
     */
    function contentLine(result) {
        return result.content === 'intact' ? t('card.content.intact') : null;
    }

    /**
     * The signed figures as label and exact value, in SIGNED_FIGURE_LABELS order; the card rounds them down.
     * @param {Object<string, *>|null} figures
     * @returns {{label: string, value: string}[]}
     */
    function signedFigureLines(figures) {
        if (!figures) return [];
        return Object.keys(SIGNED_FIGURE_LABELS).filter((key) => figures[key] != null).map((key) => {
            const value = figures[key];
            let text;
            if (key === 'period') text = cardPeriodName(value.kind, value.from);
            else if (key === 'before_fix_period') text = cardPeriodName(figures.period.kind, value.from);
            else if (key.endsWith('_permille')) text = pct(value / 1000, 1);
            else if (key.endsWith('_ms')) text = t('card.figure_ms', {n: num(value)});
            else text = num(value);
            return {label: SIGNED_FIGURE_LABELS[key], value: text};
        });
    }

    /**
     * The revoke confirmation. The date bounds nothing: the check page reads every card of a revoked
     * key as revoked, and names the date as the day since when the key may have been exposed.
     * The consequence is the alert; the DNS steps are what to do next, read apart from it.
     * @param {string} date yyyy-MM-dd @param {boolean} isCurrent @param {string|null} domain
     * @returns {{alert: string, step: string}}
     */
    function revokeText(date, isCurrent, domain) {
        // English keeps the ISO day it always showed; French reads it in words, which never breaks at a hyphen.
        const alert = t('card.revoke.alert', {date: LANG === 'fr' ? day(date, {withYear: true}) : date});
        const step = domain ? t('card.revoke.step.domain', {domain: domain}) : t('card.revoke.step');
        if (!isCurrent) return {alert: alert, step: step};
        return {
            alert: alert,
            step: step + ' ' + (domain ? t('card.revoke.next.domain', {domain: domain}) : t('card.revoke.next'))
        };
    }

    /** What the check page prints for the card: the signer only once a check found the key. */
    function levelLine(attached, domain) {
        return t(attached ? 'card.level.attached' : 'card.level.unattached', {domain: domain});
    }

    /** Where the card's environment comes from, and what it covers. */
    function cardEnvironmentLine(environment, view) {
        return t('card.env.' + (environment ? 'one' : 'all') + '.' + (view === 'pilotage' ? 'pilotage' : 'tech'),
            {environment: environment});
    }

    // Rounded down, so the screen never shows more than the card will sign.
    function cardVolume(value) {
        if (value < 10000) return num(value);
        if (value < 1000000) return t('card.volume.thousand', {n: Math.floor(value / 1000)});
        const millions = Math.floor(value / 100000) / 10;
        return t('card.volume.million.' + plural(Math.floor(millions)), {n: decimal(millions)});
    }

    /**
     * What a fix row says the card prints: the volume before the fix, or the figure a problem with
     * no volume cites instead. `period` names the period before the fix, the Hub's English name when omitted.
     * @param {*} fix @param {string} [period]
     * @returns {{figure: string|null, text: string}}
     */
    function cardFixLine(fix, period) {
        if (fix.before_fix_ops !== null && fix.before_fix_ops !== undefined) {
            return {
                figure: cardVolume(fix.before_fix_ops),
                text: t(fix.before_fix_ops >= 1000000 ? 'card.fix.volume.million' : 'card.fix.volume',
                    {period: period ?? fix.before_period_name})
            };
        }
        const type = String(fix.finding_type);
        let key = 'card.fix.calls';
        if (type.indexOf('slow_') === 0) key = 'card.fix.slow';
        else if (type === 'pool_saturation') key = 'card.fix.pool';
        else if (type === 'serialized_calls') key = 'card.fix.serialized';
        return {figure: null, text: t(key)};
    }

    /** The languages the launcher is written in, English first: it is the fallback. */
    const LANGS = ['en', 'fr'];
    const LANG_KEY = 'perf-sentinel:lang';
    /** @type {'en' | 'fr'} */
    let LANG = 'en';
    /** @type {Array<(lang: 'en' | 'fr') => void>} */
    const LANG_LISTENERS = [];

    /** @returns {'en' | 'fr'} */
    function lang() {
        return LANG;
    }

    /**
     * Shows `next`, keeps the choice when storage allows it and tells every listener. An unknown
     * language changes nothing. `persist` false adopts a language nobody chose (the browser's
     * default at boot), so a later change of browser language is still followed.
     * @param {string} next
     * @param {boolean} [persist]
     * @returns {void}
     */
    function setLang(next, persist = true) {
        if (LANGS.indexOf(next) < 0) return;
        LANG = /** @type {'en' | 'fr'} */ (next);
        if (persist) {
            try {
                global.localStorage.setItem(LANG_KEY, LANG);
            } catch (error) {
                // Storage off or absent: the choice lasts as long as the page.
            }
            // The sign-in and device pages a redirect reaches carry no lang: HubLang.Of reads this cookie.
            if (typeof document !== 'undefined') document.cookie = 'perf-sentinel-lang=' + LANG + ';max-age=31536000;samesite=lax';
        }
        if (typeof document !== 'undefined') document.documentElement.lang = LANG;
        LANG_LISTENERS.forEach(listener => listener(LANG));
    }

    /**
     * @param {(lang: 'en' | 'fr') => void} listener
     * @returns {void}
     */
    function onLang(listener) {
        LANG_LISTENERS.push(listener);
    }

    /**
     * The language to show: the stored choice when it is one, else the first `fr` or `en` variant
     * the browser lists, else English.
     * @param {readonly string[] | undefined} languages `navigator.languages`
     * @param {string | null} stored
     * @returns {'en' | 'fr'}
     */
    function pickLang(languages, stored) {
        if (LANGS.indexOf(stored) >= 0) return /** @type {'en' | 'fr'} */ (stored);
        for (const tag of languages || []) {
            const base = String(tag).toLowerCase().split('-')[0];
            if (LANGS.indexOf(base) >= 0) return /** @type {'en' | 'fr'} */ (base);
        }
        return 'en';
    }

    /**
     * @param {string} language @param {string} key
     * @returns {string | undefined}
     */
    function textIn(language, key) {
        const table = (global.PSL_TEXT || {})[language] || {};
        return Object.prototype.hasOwnProperty.call(table, key) ? table[key] : undefined;
    }

    /**
     * Whether `key` has a text to show, in the shown language or in English. Never throws.
     * @param {string} key
     * @returns {boolean}
     */
    function hasText(key) {
        return textIn(LANG, key) !== undefined || textIn('en', key) !== undefined;
    }

    /**
     * The text of `key` in the shown language, English when the shown one lacks it, with each
     * `{name}` replaced from `vars`. The tables are read here rather than at load, so a page that
     * loads launcher.js first still finds them.
     * @param {string} key
     * @param {Record<string, string | number>} [vars]
     * @returns {string}
     */
    function t(key, vars) {
        return tIn(LANG, key, vars);
    }

    /**
     * `t` in `language` rather than the shown one.
     * @param {string} language @param {string} key
     * @param {Record<string, string | number>} [vars]
     * @returns {string}
     */
    function tIn(language, key, vars) {
        const value = textIn(language, key) ?? textIn('en', key);
        if (value === undefined) {
            // Under Node (the tests) a key nobody wrote is a defect; on a page it shows as itself.
            if (typeof document === 'undefined') throw new Error('no text for ' + key);
            return key;
        }
        if (!vars) return value;
        return value.replace(/\{(\w+)\}/g, (whole, name) => name in vars ? String(vars[name]) : whole);
    }

    /**
     * What a refusal says in the shown language: the sentence of its code when the shown table has
     * one, else the Hub's English detail, else `fallback` or the generic sentence. English keeps
     * the detail first, which can name more than the table's general sentence.
     * @param {{code?: string | null, detail?: string | null} | null | undefined} payload
     * @param {number} [status]
     * @param {string} [fallback]
     * @returns {string}
     */
    function problemText(payload, status, fallback) {
        const problem = payload || {};
        const key = 'problem.' + problem.code;
        if (problem.code && textIn(LANG, key) !== undefined && (LANG !== 'en' || !problem.detail)) {
            return t(key, /** @type {Record<string, string | number>} */ (/** @type {unknown} */ (problem)));
        }
        if (problem.detail) return String(problem.detail);
        return fallback ?? t('problem.generic', {status: status ?? 0});
    }

    /**
     * @param {number} n
     * @param {Intl.NumberFormatOptions} [options]
     * @returns {string}
     */
    function num(n, options) {
        return n.toLocaleString(LANG === 'fr' ? 'fr-FR' : 'en-US', options);
    }

    /**
     * A number already written with a point, with the decimal comma in French.
     * @param {string | number} text
     * @returns {string}
     */
    function decimal(text) {
        return LANG === 'fr' ? String(text).replace('.', ',') : String(text);
    }

    /**
     * A ratio as a percentage with `digits` decimals: `3.1%` in English, `3,1 %` in French.
     * @param {number} ratio
     * @param {number} [digits]
     * @returns {string}
     */
    function pct(ratio, digits = 0) {
        if (LANG !== 'fr') return (ratio * 100).toFixed(digits) + '%';
        // The narrow space by hand: ICU builds disagree on the one before `%` (Node 24 writes U+00A0).
        return num(ratio * 100, {minimumFractionDigits: digits, maximumFractionDigits: digits}) + ' %';
    }

    /**
     * A `yyyy-MM-dd` day in words, UTC: `29 October 2026` in English, as cardDayText writes it.
     * @param {string} iso
     * @param {{withYear?: boolean, short?: boolean, language?: string}} [options]
     * @returns {string}
     */
    function day(iso, {withYear = false, short = false, language = LANG} = {}) {
        if (language === 'fr') {
            // French writes the first of the month as an ordinal, 1er, which Intl leaves to us.
            return new Intl.DateTimeFormat('fr-FR', {
                day: 'numeric', month: short ? 'short' : 'long', year: withYear ? 'numeric' : undefined, timeZone: 'UTC'
            }).format(cardDate(iso)).replace(/^1(?= )/, '1er');
        }
        if (!short) return cardDayText(iso, withYear);
        const date = cardDate(iso);
        return date.getUTCDate() + ' ' + CARD_MONTHS[date.getUTCMonth()].slice(0, 3) + (withYear ? ' ' + date.getUTCFullYear() : '');
    }

    /**
     * The plural form of `n` in the shown language, the suffix of a `.one` / `.other` key pair:
     * English writes one for 1 alone, French for 0 and 1.
     * @param {number} n
     * @returns {'one' | 'other'}
     */
    function plural(n) {
        return (LANG === 'fr' ? n <= 1 : n === 1) ? 'one' : 'other';
    }

    /**
     * A moment's local time to the minute, `14:05`: the time dateTime writes, without the day.
     * @param {number} ms
     * @returns {string}
     */
    function time(ms) {
        if (LANG !== 'fr') return dtHuman(ms).slice(11);
        return new Intl.DateTimeFormat('fr-FR', {hour: '2-digit', minute: '2-digit'}).format(new Date(ms));
    }

    /**
     * A moment in local time to the minute: `2026-10-29 14:05` in English, as dtHuman writes it.
     * @param {number} ms
     * @returns {string}
     */
    function dateTime(ms) {
        if (LANG !== 'fr') return dtHuman(ms);
        return new Intl.DateTimeFormat('fr-FR', {
            day: '2-digit', month: '2-digit', year: 'numeric', hour: '2-digit', minute: '2-digit'
        }).format(new Date(ms));
    }

    /**
     * The string literals of `source`, each with where it starts in a copy of the source that has
     * no comments and every literal masked, so the code before a literal can be read as context.
     * A literal that is a bare token stays in that copy unquoted, which keeps `'aria-label': x`
     * and `'class': x` readable as keys. Template literals keep their `${}` and the literals inside
     * those are listed too.
     * @param {string} source
     * @returns {{code: string, literals: Array<{text: string, at: number}>}}
     */
    function scanLiterals(source) {
        /** @type {Array<{text: string, at: number}>} */
        const literals = [];
        let code = '';
        let i = 0;

        function regexAllowed() {
            const before = code.trimEnd();
            return before === '' || '(,=:[!&|?{};+-*%<>~^'.indexOf(before[before.length - 1]) >= 0
                || /\b(?:return|typeof|case|in|of|void|delete|throw|new|else|do)$/.test(before);
        }

        function skipRegex() {
            let inClass = false;
            i++;
            while (i < source.length && source[i] !== '\n') {
                const c = source[i];
                if (c === '\\') i++;
                else if (c === '[') inClass = true;
                else if (c === ']') inClass = false;
                else if (c === '/' && !inClass) break;
                i++;
            }
            i++;
            while (/[a-z]/.test(source[i] || '')) i++;
            code += 'R';
        }

        /** @param {string} quote */
        function quoted(quote) {
            let text = '';
            i++;
            while (i < source.length && source[i] !== quote && source[i] !== '\n') {
                if (source[i] === '\\') i++;
                text += source[i] ?? '';
                i++;
            }
            i++;
            literals.push({text: text, at: code.length});
            code += /^[\w-]+$/.test(text) ? text : '""';
        }

        function template() {
            const literal = {text: '', at: code.length};
            literals.push(literal);
            i++;
            while (i < source.length && source[i] !== '`') {
                if (source[i] === '\\') {
                    literal.text += source.slice(i, i + 2);
                    i += 2;
                } else if (source[i] === '$' && source[i + 1] === '{') {
                    const from = i;
                    i += 2;
                    scan(true);
                    literal.text += source.slice(from, i);
                } else {
                    literal.text += source[i++];
                }
            }
            i++;
            code += '""';
        }

        /** @param {boolean} untilBrace stop after the `}` that closes a template's `${` */
        function scan(untilBrace) {
            let depth = 0;
            while (i < source.length) {
                const c = source[i], next = source[i + 1];
                if (c === '/' && next === '/') {
                    const end = source.indexOf('\n', i);
                    i = end < 0 ? source.length : end;
                } else if (c === '/' && next === '*') {
                    const end = source.indexOf('*/', i + 2);
                    i = end < 0 ? source.length : end + 2;
                } else if (c === '\'' || c === '"') {
                    quoted(c);
                } else if (c === '`') {
                    template();
                } else if (c === '/' && regexAllowed()) {
                    skipRegex();
                } else {
                    if (untilBrace && c === '{') depth++;
                    if (untilBrace && c === '}' && depth-- === 0) {
                        i++;
                        return;
                    }
                    code += c;
                    i++;
                }
            }
        }

        scan(false);
        return {code: code, literals: literals};
    }

    /**
     * From `open`, the index of the bracket that closes it.
     * @param {string} code @param {number} open
     * @returns {number}
     */
    function closingBracket(code, open) {
        const pairs = /** @type {Record<string, string>} */ ({'(': ')', '{': '}'});
        const opener = code[open], closer = pairs[opener];
        let depth = 0;
        for (let i = open; i < code.length; i++) {
            if (code[i] === opener) depth++;
            else if (code[i] === closer && --depth === 0) return i;
        }
        return code.length;
    }

    /** Attributes whose value is an identifier, never text a person reads. */
    const TECHNICAL_KEY = '(?:class|className|id|href|src|type|role|name|for|rel)';
    /** The code before a literal that makes it technical, whatever its shape. */
    const TECHNICAL_CONTEXT = [
        new RegExp('(?:^|[\\s{,(])' + TECHNICAL_KEY + '\\s*:\\s*(?:[^;,={}()]*?[?:]\\s*)?$'),
        new RegExp('\\.' + TECHNICAL_KEY + '\\s*=\\s*(?:[^;,={}()]*?[?:]\\s*)?$'),
        new RegExp('\\bsetAttribute\\(\\s*' + TECHNICAL_KEY + '\\s*,\\s*$'),
        /\bclassList\.\w+\(\s*(?:[^;()]*,\s*)?$/,
        /\b(?:querySelector|querySelectorAll|closest|matches|getElementById|matchMedia)\(\s*$/,
        /\b(?:add|remove)EventListener\(\s*$/,
        /\bt\(\s*$/
    ];

    /**
     * Whether a literal reads as a sentence for a person: two words or more, one of them a
     * lower-case word of three letters or more, and not a URL, a selector, a key or CSS.
     * @param {string} text
     * @returns {boolean}
     */
    function isProse(text) {
        if (/:\/\//.test(text) || /^[.#[]/.test(text) || /^[a-z0-9_]+(\.[a-z0-9_-]+)+$/.test(text)) return false;
        if (text.indexOf(':') >= 0 && text.indexOf(';') >= 0) return false;
        const words = text.replace(/\$\{[^}]*\}/g, 'X').split(/\s+/).filter(Boolean);
        return words.length >= 2 && words.some(word => /^[a-z]{3,}$/.test(word.replace(/[^A-Za-z]/g, '')));
    }

    /**
     * The prose literals inside the functions named `functionNames` in `source`: the guard that
     * keeps displayed text out of the code and in the text tables. A test helper, the page never
     * calls it. A literal is skipped by its context first (a class, an id, a selector, an event, a
     * `t(` key), then by its shape.
     * @param {string} source
     * @param {string[]} functionNames
     * @returns {string[]}
     */
    function literalsIn(source, functionNames) {
        const {code, literals} = scanLiterals(source);
        /** @type {Array<[number, number]>} */
        const bodies = [];
        for (const name of functionNames) {
            const header = new RegExp('\\bfunction\\s+' + name + '\\s*\\(', 'g');
            let match;
            while ((match = header.exec(code)) !== null) {
                const params = closingBracket(code, match.index + match[0].length - 1);
                const open = code.indexOf('{', params);
                if (open >= 0) bodies.push([open, closingBracket(code, open)]);
            }
        }
        return literals
            .filter(literal => bodies.some(([open, close]) => literal.at > open && literal.at < close))
            .filter(literal => !TECHNICAL_CONTEXT.some(pattern => pattern.test(code.slice(Math.max(0, literal.at - 120), literal.at))))
            .filter(literal => isProse(literal.text))
            .map(literal => literal.text);
    }

    global.PSL = {
        setVersions,
        get ENGINE() {
            return ENGINE;
        },
        get HUB() {
            return HUB;
        },
        ERRORS,
        READ_ERRORS,
        ERROR_TITLES,
        sourceHealth,
        runReadOutcome,
        retryDelay,
        KIND_LABEL,
        dur,
        durPrecise,
        durMinutes,
        durParts,
        splitByKind,
        clock,
        parseDur,
        humanDur,
        dtLocal,
        dtHuman,
        bytes,
        vparts,
        vcmp,
        versionGap,
        skew,
        detector,
        statusKey,
        argsLine,
        weightBand,
        shq,
        psq,
        SHELLS,
        shellById,
        defaultShell,
        exportLine,
        analysisCommand,
        monitorCommand,
        monitorKeyHint,
        detectionToml,
        quotedForShell,
        lightState,
        mergeableView,
        mergeLight,
        refreshPlan,
        releaseUrl,
        openFolds,
        daemonGroupFolds,
        hubReleaseUrl,
        updateState,
        knownShell,
        CHART_PAGE,
        CHART_COORDINATE,
        gaugeTone,
        gaugeMove,
        INCIDENT_KIND_LABEL,
        FINDING_TYPE_LABEL,
        incidentCapture,
        findingPhase,
        sortFindings,
        moreFiltersLabel,
        findingTemplate,
        findingImpact,
        findingMatches,
        ackCommand,
        phaseLabel,
        INCIDENT_READ_STATE,
        incidentFilterState,
        incidentsCopy,
        incidentHandoffHash,
        readHandoff,
        CORRELATION_RANGES,
        readCorrelationPair,
        correlationWindowKey,
        pairLabel,
        episodeEndNote,
        correlationPairs,
        spansOverlap,
        episodeHandoffHash,
        pairTraceHash,
        readTraceHandoff,
        timelineMarks,
        lagText,
        episodeSpan,
        ackRouteHash,
        ackEntryHash,
        readAckRoute,
        ackRows,
        formatDetections,
        detectionWindow,
        gapLabel,
        ackExpiry,
        ackExpiryLine,
        ackChecked,
        ackPlan,
        ackBody,
        ackSummary,
        shellBanner,
        screenOf,
        readDisclosureId,
        periodRail,
        periodLabel,
        monthName,
        coverageNotch,
        checkRows,
        hostRows,
        hostKwh,
        verifyCommand,
        publishCommand,
        downloadAs,
        formFromRequest,
        permits,
        describePattern,
        accessExpiry,
        accessSignedIn,
        get ACCESS_NO_REACH() {
            return t('access.no_reach');
        },
        accessReachRows,
        coverageSnippet,
        accessRequestText,
        sourceCountLabel,
        revokeOutcome,
        tokenRows,
        tokenRevokeRequest,
        deviceIntro,
        deviceRequestLine,
        deviceNetworkAlert,
        deviceApproveStep,
        ownerLabel,
        RECAP_LABELS,
        RECAP_HELP,
        serviceParts,
        namesService,
        readRecapRoute,
        recapHash,
        recapPath,
        recapReportLabel,
        RECAP_REPORTS_COPY,
        recapPlain,
        recapFreezeText,
        recapErrorText,
        recapTruncatedText,
        recapSourcesNote,
        recapLateNote,
        recapReportStatus,
        recapDeliveryText,
        recapOnDemandBody,
        co2Parts,
        co2Text,
        co2Tail,
        trendText,
        hasAvoidableVolume,
        dividesBefore,
        confirmationLabel,
        periodText,
        incompleteText,
        notesText,
        httpUrl,
        localTime,
        isoUtc,
        saveBlob,
        CARD_PERIODS,
        CARD_PERIOD_LABELS,
        CARD_ANGLES,
        CARD_ANGLE_NAMES,
        CARD_ANGLE_LINES,
        CARD_FORMATS,
        CARD_FORMAT_LABELS,
        CARD_FORMAT_NOTES,
        previewFormat,
        cardBusy,
        CARD_COPY,
        ATTACH_REASONS,
        ATTACH_VERDICTS,
        SIGNED_FIGURE_LABELS,
        cardPeriodStart,
        stepPeriod,
        lastClosedPeriod,
        canStepForward,
        cardPeriodName,
        cardRefusalText,
        cardLang,
        CARD_COUNTRIES,
        cardCountry,
        cardCountryWire,
        cardDayText,
        cardRangeText,
        cardBody,
        cardOptionsPath,
        canRememberLabel,
        rememberState,
        cardFieldProblem,
        isPreviousRefusal,
        cardContactText,
        attachmentText,
        contentLine,
        signedFigureLines,
        revokeText,
        levelLine,
        cardEnvironmentLine,
        cardFixLine,
        lang,
        setLang,
        onLang,
        pickLang,
        hasText,
        t,
        problemText,
        num,
        decimal,
        pct,
        day,
        dateTime,
        time,
        plural,
        sourceCount,
        literalsIn
    };
})(globalThis);
