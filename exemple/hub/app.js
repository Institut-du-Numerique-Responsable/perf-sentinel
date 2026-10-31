/**
 * perf-sentinel Hub launcher — shell, state and rendering.
 *
 * Classic script, no build step. Pure logic lives in launcher.js as `PSL`.
 *
 * Nothing from the server is ever written with innerHTML: every displayed
 * string is a text node, and nothing in this design needs rich HTML from data.
 */
(function () {
    'use strict';

    const PSL = globalThis.PSL;
    const THEME_KEY = 'perf-sentinel:theme';
    const THEME_POSITIONS = ['auto', 'light', 'dark'];
    const t = PSL.t;

    /** Glyph paths lifted from the dashboard's themeIcon(), not redrawn. */
    const THEME_GLYPHS = {
        auto: [['rect', {x: '3', y: '4', width: '18', height: '13', rx: '2'}], ['path', {d: 'M8 21h8M12 17v4'}]],
        light: [['circle', {cx: '12', cy: '12', r: '4'}], ['path', {
            d: 'M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4'
        }]],
        dark: [['path', {d: 'M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z'}]]
    };

    const state = {
        themePosition: document.documentElement.getAttribute('data-theme-position') || 'auto',
        screen: 'new',
        status: null,
        // The HTTP status /api/status failed with, which tells "no grant" from "down".
        statusError: undefined,
        sources: null,
        sourcesError: false,
        // GET /api/access, null when access is off (the route answers 404) or failed.
        access: null,
        // GET /api/tokens, read only with access on; null until it answers or when it failed.
        tokens: null,
        tokensFailed: false,
        // What the last Revoke did, {card, ok, text}, shown in the card it was pressed in.
        tokensNotice: null,
        // Set once the first screen has drawn: from then on a route change moves the focus.
        routed: false,
        loading: true,
        run: null,
        runError: false,
        // Reads of the open run that failed in a row, for a reason other than its absence.
        runReadFailures: 0,
        runTimer: null,
        noteTimer: null,
        // Every node whose text is a duration measured against the current moment,
        // and the single ticker that rewrites them. A countdown drawn once is wrong
        // a second later, and re-rendering the screen to refresh it would replace
        // the report iframe and reload the report underneath the reader.
        liveDurations: [],
        durationTicker: null,
        runs: null,
        // Which daemon rows are folded open, and what each one answered. Kept
        // across a render so a rebuilt table comes back the way it was left.
        daemonOpen: {},
        daemonViews: {},
        daemonSettingsOpen: {},
        daemonGroupOpen: {},
        // Poll interval per source in ms, 0 for off, and the one-second ticker
        // that drives both the countdown and the fetch.
        daemonRefreshMs: {},
        daemonTickers: {},
        // The read's own deadline, separate from the one-second ticker that writes
        // the countdown: a read timed off that ticker fires on the next whole
        // second, which is up to a second after the disc has already closed.
        daemonCycleTimers: {},
        daemonCycleAt: {},
        daemonInFlight: {},
        // When THIS browser last adopted a reading, on its own clock: subtracting
        // the Hub's observed_at_ms from Date.now() would bake the clock skew
        // between the two machines into "Read X ago".
        daemonReadAt: {},
        // When the last FULL read happened: light ticks carry the gauges alone,
        // and once a minute the full read re-syncs hints, state and settings age.
        daemonFullReadAt: {},
        daemonTerminalOpen: {},
        // What each gauge moved by on the read just adopted, shown once beside the
        // figure and then gone. Consumed by the render, so a rebuild for any other
        // reason does not replay a move that already had its moment.
        daemonMoves: {},
        // Folds that belong to a screen rather than to a source. Kept in the same
        // record, since a reader does not care which of the two a fold is.
        panelOpen: {},
        // The incidents screen: the rows loaded so far, whether the last page
        // was short (nothing older to ask for), the filters, every service and
        // namespace seen across loads so the selects outlive their own filter,
        // and what each unfolded row answered.
        incidents: null,
        // Set by the first read of the fleet that completes, failed or not, and
        // never reset: the filters it releases keep their lists across refreshes.
        incidentsRead: false,
        incidentsError: false,
        incidentsDone: false,
        // Set while a live read of the fleet is in flight, so the read-now
        // button cannot be pressed twice into the same request.
        incidentsReading: false,
        // Keyed by the query parameter each one becomes on /api/incidents.
        incidentFilter: {service: '', namespace: '', kind: '', environment: '', source_id: ''},
        incidentServices: [],
        incidentNamespaces: [],
        incidentDetails: {},
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
        // The recap screen: the snapshot GET /api/recap answered (null while it loads), the status
        // of a failed read, whether the priorities are unfolded past ten, the sentence a refused
        // write left in its row by signature, the rows with a write in flight, the row whose
        // endpoint is being named, and the number of the latest read so an old one cannot land.
        recap: null,
        recapError: null,
        recapShowAll: false,
        recapRowError: {},
        recapBusy: {},
        recapNaming: null,
        // What was typed in the name field when the Hub refused it, so a refusal does not eat it.
        recapNameDraft: null,
        recapSeq: 0,
        // The frozen reports under the recap: the list GET /api/recap/reports answered, null when the
        // Hub refused it (403, the section stays hidden) or while it loads, 'error' when it failed
        // otherwise; whether a generate is in flight and the sentence a refused one left.
        recapReports: null,
        recapGenerating: false,
        recapGenerateError: null,
        // The share card form, made on first use (cardState).
        card: null,
        // GET /api/disclosures, null while it loads.
        disclosures: null,
        disclosuresError: false,
        // GET /api/disclosures/{id} and the report.json it serves, for the detail screen.
        disclosure: null,
        disclosureReport: null,
        disclosureError: false,
        // What the last action on the detail did, and a share link's token, shown once.
        disclosureNote: null,
        disclosureLink: null,
        disclosureBusy: false,
        withdrawOpen: false,
        incidentCorrelations: {},
        // A trace an episode handed to New analysis, with the pair's other one.
        traceHandoff: null,
        // The ack page: the finding its link names, the form, and what the last
        // submit came to. Null until a link opens it.
        ack: null,
        // The route an in-app Ack link was followed from, for the page's way back.
        ackReturn: null,
        // How an incident's frozen findings are ordered: worst, then most seen, first.
        findingSort: {key: 'severity', dir: 'desc'},
        incidentFiltersOpen: false,
        // What was typed above each incident's findings, by incident key.
        findingQuery: {},
        // The incident a `#/new?from=…` link pre-filled the form from, or null.
        handoff: null,
        // Which shell every printed command is spelled for.
        shell: 'posix',
        terminalSig: null,
        form: {
            sourceId: null,
            mode: 'service',
            service: '',
            traceId: '',
            rangeMode: 'relative',
            lookback: '1h',
            fromMs: Date.now() - 3600000,
            toMs: Date.now(),
            customQty: 90,
            customUnit: 'm',
            detection: {},
            pickerOpen: false,
            maxTraces: 100,
            ackUnreachable: false,
            ackHeavy: false
        }
    };

    // ------------------------------------------------------------ DOM helpers

    function el(tag, attrs, children) {
        const node = document.createElement(tag);
        Object.keys(attrs || {}).forEach(function (key) {
            if (key === 'class') node.className = attrs[key];
            else if (key === 'text') node.textContent = attrs[key];
            else if (attrs[key] != null) node.setAttribute(key, String(attrs[key]));
        });
        (children || []).forEach(function (child) {
            if (child) node.appendChild(child);
        });
        return node;
    }

    function svg(paths, size) {
        const node = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
        node.setAttribute('viewBox', '0 0 24 24');
        node.setAttribute('fill', 'none');
        node.setAttribute('stroke', 'currentColor');
        node.setAttribute('stroke-width', '1.9');
        node.setAttribute('stroke-linecap', 'round');
        node.setAttribute('stroke-linejoin', 'round');
        node.setAttribute('aria-hidden', 'true');
        if (size) {
            node.setAttribute('width', String(size));
            node.setAttribute('height', String(size));
        }
        paths.forEach(function (spec) {
            const shape = document.createElementNS('http://www.w3.org/2000/svg', spec[0]);
            Object.keys(spec[1]).forEach(function (key) {
                shape.setAttribute(key, spec[1][key]);
            });
            node.appendChild(shape);
        });
        return node;
    }

    // Write-only: the read happens in the inline script, before this file loads.
    // sessionStorage throws in Safari private mode and under some enterprise
    // policies, and a theme is not worth an error.
    function store(area, key, value) {
        try {
            globalThis[area].setItem(key, value);
        } catch (error) {
            // Nothing to do: the position still applies to this page.
        }
    }

    // ---------------------------------------------------------------- theme

    function resolveTheme(position) {
        if (position !== 'auto') return position;
        return matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
    }

    function applyTheme(animate) {
        const root = document.documentElement;
        root.setAttribute('data-theme', resolveTheme(state.themePosition));
        root.setAttribute('data-theme-position', state.themePosition);
        // Both stores: localStorage so the position survives the tab, sessionStorage
        // because the rendered dashboard reads that exact key from this origin.
        store('localStorage', THEME_KEY, state.themePosition);
        store('sessionStorage', THEME_KEY, state.themePosition);

        const button = document.getElementById('theme-toggle');
        const glyph = document.getElementById('theme-glyph');
        const label = t('shell.theme.' + state.themePosition);
        document.getElementById('theme-label').textContent = label;
        button.setAttribute('aria-label', t('shell.theme.button', {label: label}));
        glyph.replaceChildren(svg(THEME_GLYPHS[state.themePosition], 15));
        if (!animate) return;
        // Two identical keyframes alternated, to force the animation to restart.
        button.setAttribute('data-spin', button.getAttribute('data-spin') === 'a' ? 'b' : 'a');
    }

    function initTheme() {
        document.getElementById('theme-toggle').addEventListener('click', function () {
            const next = (THEME_POSITIONS.indexOf(state.themePosition) + 1) % THEME_POSITIONS.length;
            state.themePosition = THEME_POSITIONS[next];
            applyTheme(true);
        });
        // An OS change re-resolves live and never animates, or the theme would
        // spin by itself at sunset.
        matchMedia('(prefers-color-scheme: light)').addEventListener('change', function () {
            if (state.themePosition === 'auto') applyTheme(false);
        });
        applyTheme(false);
    }

    // ------------------------------------------------------------- language

    /**
     * Writes the tables' text into every `[data-i18n]` under `root`, and each attribute a
     * `data-i18n-attr="aria-label:key;title:key"` names. index.html keeps its English as the
     * text a page shows before any script runs.
     */
    function applyStaticText(root) {
        root.querySelectorAll('[data-i18n]').forEach(function (node) {
            node.textContent = t(node.getAttribute('data-i18n'));
        });
        root.querySelectorAll('[data-i18n-attr]').forEach(function (node) {
            node.getAttribute('data-i18n-attr').split(';').forEach(function (pair) {
                const parts = pair.split(':');
                node.setAttribute(parts[0], t(parts[1]));
            });
        });
    }

    /** Marks a block the engine wrote, which stays English on a French screen. */
    function engineText(node) {
        node.setAttribute('lang', 'en');
        node.classList.add('engine-text');
        return node;
    }

    /**
     * The pre-paint script in index.html already chose the language and wrote it on <html>.
     * A switch redraws in place: the route, the state and the form stay as they are.
     */
    function initLang() {
        // The pre-paint script already picked it: adopt it, do not store it as a choice.
        PSL.setLang(document.documentElement.lang, false);
        applyStaticText(document);
        document.getElementById('lang-toggle').addEventListener('click', function () {
            PSL.setLang(PSL.lang() === 'fr' ? 'en' : 'fr');
            // A card that follows the Hub's language no longer matches its preview.
            if (state.card && !state.card.lang) cardMarkStale(state.card);
            applyStaticText(document);
            applyTheme(false);
            // The shell's own lines are drawn from the status, which a loading page does not have yet.
            if (!state.loading) renderShell();
            // The focus is on this button, outside #main, so the redraw cannot drop it.
            render();
        });
    }

    // ----------------------------------------------------------------- data

    /**
     * Under Hub:Auth a lapsed session answers 401 rather than a redirect a fetch
     * cannot follow to the provider. Reloading the page is what signs back in.
     */
    function signInOn401(response) {
        if (response.status === 401) location.reload();
        return response;
    }

    function fetchJson(path, method) {
        return fetch(path, {
            method: method,
            headers: {accept: 'application/json'}
        }).then(signInOn401).then(function (response) {
            if (!response.ok) {
                // The parsed problem rides on the error, so a screen can say it in the shown language.
                return response.json().catch(function () {
                    return null;
                }).then(function (problem) {
                    const error = new Error(path + ' answered ' + response.status);
                    error.status = response.status;
                    error.problem = problem;
                    throw error;
                });
            }
            return response.json();
        });
    }

    function getJson(path) {
        return fetchJson(path, 'GET');
    }

    /** A POST because the route writes: it reads the daemons before it answers. */
    function postJson(path) {
        return fetchJson(path, 'POST');
    }

    function loadShell() {
        return Promise.all([
            getJson('/api/status').catch(function (error) {
                state.statusError = error.status;
                return null;
            }),
            getJson('/api/sources').catch(function () {
                return 'error';
            }),
            // A 404 is a Hub with access off: the tab stays hidden.
            getJson('/api/access').catch(function () {
                return null;
            })
        ]).then(function (results) {
            state.status = results[0];
            state.sourcesError = results[1] === 'error';
            state.sources = state.sourcesError ? null : results[1];
            state.access = results[2];
            state.loading = false;
            if (state.access) loadTokens();
            const runnable = runSources();
            if (runnable.length > 0 && state.form.sourceId === null) {
                // The one they were on last, if it is still configured. Falling back to
                // the first reachable one is what a first visit gets.
                const remembered = rememberedSource();
                const kept = runnable.find(function (source) {
                    return source.id === remembered;
                });
                const usable = runnable.find(function (source) {
                    return source.reachable;
                });
                state.form.sourceId = (kept || usable || runnable[0]).id;
            }
            renderShell();
            onRoute();
        });
    }

    /** The terminal tokens the access page lists: the caller's own, or everyone's for an admin. */
    function loadTokens() {
        return getJson('/api/tokens').then(function (tokens) {
            state.tokens = tokens;
            state.tokensFailed = false;
        }).catch(function () {
            state.tokens = null;
            state.tokensFailed = true;
        }).then(function () {
            if (state.screen === 'access' || state.screen === 'coverage') render();
        });
    }

    /**
     * The sources alone, for a screen whose own action moved their state. A
     * failure is left silent: the incidents that came back are still real, only
     * the read stamps under them stay as they were.
     */
    function reloadSources() {
        return getJson('/api/sources').then(function (sources) {
            state.sources = sources;
            state.sourcesError = false;
            renderSourcesBadge();
        }).catch(function () {
            void 0;
        });
    }

    // Folds outlive the page. A reader who opened a row, its settings and two of
    // its groups should find all four the way they left them, so the four maps
    // are one record rather than four, written whenever one of them changes.
    const FOLD_STORAGE_KEY = 'perf-sentinel-hub.folds';
    // Its own key rather than a field in the fold record: a chosen source is not
    // a fold, and one name per thing survives the next thing worth remembering.
    const SOURCE_STORAGE_KEY = 'perf-sentinel-hub.source';
    const SHELL_STORAGE_KEY = 'perf-sentinel-hub.shell';

    /**
     * The shell every printed command is spelled for. The reader's own choice
     * once they make one, and until then the one their platform opens with.
     */
    function restoreShell() {
        let stored = null;
        try {
            stored = localStorage.getItem(SHELL_STORAGE_KEY);
        } catch (error) {
            stored = null;
        }
        const platform = (navigator.userAgentData && navigator.userAgentData.platform)
            || navigator.platform;
        // A stored id that no longer names a shell is worth no more than no id at
        // all, so it falls back to the platform rather than to the first entry.
        state.shell = PSL.knownShell(stored) || PSL.defaultShell(platform);
    }

    function saveShell(id) {
        try {
            localStorage.setItem(SHELL_STORAGE_KEY, id);
        } catch (error) {
            // Storage refused. The choice holds for this page and the next visit
            // opens on the platform's own shell again.
        }
    }

    function rememberedSource() {
        try {
            return localStorage.getItem(SOURCE_STORAGE_KEY);
        } catch (error) {
            return null;
        }
    }

    function saveSource(id) {
        try {
            if (id) localStorage.setItem(SOURCE_STORAGE_KEY, id);
        } catch (error) {
            // Storage refused. The next visit opens on the default source, which is
            // where every visit used to open.
        }
    }

    function restoreFolds() {
        let stored = null;
        try {
            stored = JSON.parse(localStorage.getItem(FOLD_STORAGE_KEY) || 'null');
        } catch (error) {
            // Storage refused, or held something that is not JSON. Everything starts
            // folded, which is what a first visit gets anyway.
            stored = null;
        }
        if (!stored || typeof stored !== 'object') return;
        state.daemonOpen = PSL.openFolds(stored.row);
        state.daemonSettingsOpen = PSL.openFolds(stored.settings);
        state.daemonGroupOpen = PSL.daemonGroupFolds(stored.group);
        state.daemonTerminalOpen = PSL.openFolds(stored.terminal);
        state.panelOpen = PSL.openFolds(stored.panel);
    }

    function saveFolds() {
        try {
            localStorage.setItem(FOLD_STORAGE_KEY, JSON.stringify({
                row: PSL.openFolds(state.daemonOpen),
                settings: PSL.openFolds(state.daemonSettingsOpen),
                group: PSL.openFolds(state.daemonGroupOpen),
                terminal: PSL.openFolds(state.daemonTerminalOpen),
                panel: PSL.openFolds(state.panelOpen)
            }));
        } catch (error) {
            // A full or disabled store costs the reader the memory of their folds
            // and nothing else, so there is nothing to report here.
        }
    }

    function renderShell() {
        const status = state.status;
        // Without a status there is no version to show, and "unknown" read as a broken Hub.
        // A caller no grant reaches gets no tab either: every one of them leads to a refusal.
        document.getElementById('version-chip').hidden = !status;
        document.querySelector('.shell-tabs').hidden = !status && !!PSL.shellBanner(state.statusError);
        document.getElementById('version-hub').textContent = status ? status.version : t('shell.version.unknown');
        document.getElementById('version-engine').textContent =
            status && status.engine_version ? status.engine_version : t('shell.version.none');
        if (status) PSL.setVersions(status.version, status.engine_version);
        renderUpdates();

        // The identity comes from the Hub's sign-in or the reverse proxy. With neither
        // there is nothing to show, and an empty chip is better than a fake name. With
        // access on it is the way to one's own access, refused callers included.
        const identity = document.getElementById('identity');
        const access = state.access;
        const name = (status && status.identity) || (access && (access.identity || access.subject)) || '';
        identity.hidden = !name;
        document.getElementById('identity-name').textContent = name;
        if (access) {
            identity.setAttribute('href', '#/access');
            identity.title = t('shell.identity.access');
        } else {
            identity.removeAttribute('href');
        }

        renderFleetSkew();
        renderSourcesBadge();
        document.querySelector('.shell-tab[data-screen="coverage"]').hidden = !(state.access && state.access.admin);
        // On only when the Hub has disclosures on and this caller reads them.
        document.querySelector('.shell-tab[data-screen="disclosures"]').hidden = !(state.status && state.status.disclosures);
    }

    /** Takes a chip segment out along with the separator that was added for it. */
    function dropSegment(chip, selector) {
        const existing = chip.querySelector(selector);
        if (!existing) return;
        const rule = existing.previousElementSibling;
        if (rule && rule.classList.contains('shell-version-rule')) rule.remove();
        existing.remove();
    }

    /**
     * What the Hub last heard from GitHub, and only when it is news. Absent says
     * nothing either way: the check may be off, may not have run, or may not have
     * reached anything, and none of those mean the versions here are current.
     */
    function renderUpdates() {
        const chip = document.getElementById('version-chip');
        // The rule belongs to the segment, so it goes with it. Removing one and not
        // the other leaves a separator behind on every rebuild.
        dropSegment(chip, '.shell-version-update');
        if (!state.status) return;

        const behind = [
            [t('shell.version.hub'), PSL.updateState(state.status.version, state.status.latest_hub_version),
                PSL.hubReleaseUrl()],
            [t('shell.version.engine'), PSL.updateState(state.status.engine_version, state.status.latest_engine_version),
                null]
        ].filter(function (row) {
            return row[1];
        });
        if (behind.length === 0) return;

        const segment = el('span', {
            class: 'shell-version-update',
            title: t('shell.update.title')
        }, [
            svg([['path', {d: 'M12 19V5M5 12l7-7 7 7'}]], 12),
            // Said, not left to be inferred from two numbers side by side. The Hub
            // asked and got an answer, so it can claim this much.
            el('span', {class: 'shell-version-news', text: t('shell.update.news')})
        ]);
        behind.forEach(function (row, index) {
            segment.appendChild(el('span', {text: index === 0 ? ':' : '\u00b7'}));
            segment.appendChild(el('a', {
                href: row[2] || PSL.releaseUrl(row[1].latest),
                target: '_blank',
                rel: 'noopener noreferrer',
                text: row[0] + ' ' + row[1].latest
            }));
        });
        chip.appendChild(el('span', {class: 'shell-version-rule', 'aria-hidden': 'true'}));
        chip.appendChild(segment);
    }

    /**
     * The last version segment, and it appears only when a producer is behind the
     * engine. It names the spread across the fleet, not one source.
     */
    function renderFleetSkew() {
        const chip = document.getElementById('version-chip');
        dropSegment(chip, '.shell-version-skew');
        if (!state.sources || !state.status || !state.status.engine_version) return;

        // Behind only. A fleet ahead of the Hub is a normal moment during a rollout,
        // and the arrow would have pointed from the newer version down to the older
        // one, which reads as an instruction to downgrade.
        const behind = state.sources
            .map(function (source) {
                return source.producer_version;
            })
            .filter(function (version) {
                const skew = version && PSL.skew(version);
                return skew && skew.dir === 'behind';
            });
        if (behind.length === 0) return;

        const oldest = behind.sort(PSL.vcmp)[0];
        chip.appendChild(el('span', {class: 'shell-version-rule', 'aria-hidden': 'true'}));
        chip.appendChild(el('span', {class: 'shell-version-skew'}, [
            svg([['path', {d: 'M12 4l9 16H3z'}], ['path', {d: 'M12 10v4M12 17.4v.2'}]], 12),
            el('span', {text: t('shell.skew', {oldest: oldest, engine: state.status.engine_version})})
        ]));
    }

    function renderSourcesBadge() {
        const badge = document.getElementById('sources-badge');
        const unreachable = (state.sources || []).filter(function (source) {
            return !source.reachable;
        });
        badge.hidden = unreachable.length === 0;
        badge.textContent = String(unreachable.length);
    }

    // ------------------------------------------------------------ navigation

    function currentScreen() {
        return PSL.screenOf(location.hash, !!state.access, !!(state.access && state.access.admin));
    }

    function currentRunId() {
        const match = /^#\/(?:run|report)\/([0-9a-f]{16})$/.exec(location.hash || '');
        return match ? match[1] : null;
    }

    /**
     * Marks a node's text as a live duration. `compute` is re-run every second and
     * its result written back, so a caller passes the same function it used to
     * build the initial text.
     */
    function live(node, compute) {
        if (typeof compute === 'function') state.liveDurations.push({node: node, compute: compute});
        return node;
    }

    function startDurationTicker() {
        if (!state.liveDurations.length) return;
        state.durationTicker = setInterval(function () {
            state.liveDurations.forEach(function (entry) {
                // A failed resubmission borrows this line for six seconds and stashes
                // the original on the node. Rewriting it now would wipe the error
                // before anyone read it.
                if (entry.node.dataset && entry.node.dataset.restore !== undefined) return;
                // Per entry: one compute that throws must not freeze every countdown
                // behind it in the list for the rest of the screen's life.
                try {
                    const text = entry.compute();
                    entry.node.textContent = text;
                    // Some cells carry the same string as a tooltip. Left alone it would
                    // still read the value this screen was built with.
                    if (entry.node.title) entry.node.title = text;
                } catch (error) {
                    void error;
                }
            });
        }, 1000);
    }

    function stopDurationTicker() {
        clearInterval(state.durationTicker);
        state.durationTicker = null;
        // The nodes themselves are about to be replaced, so the registry goes too.
        state.liveDurations = [];
    }

    // On a phone the tab row scrolls on its own, and a screen opened by a link or a reload has its
    // tab off to the right. Only the row moves: scrollIntoView would scroll the page too.
    function revealCurrentTab() {
        const tab = document.querySelector('.shell-tab[aria-current="page"]');
        const row = tab && tab.closest('.shell-tabs');
        if (!row) return;
        const rowBox = row.getBoundingClientRect();
        const tabBox = tab.getBoundingClientRect();
        if (tabBox.left < rowBox.left) row.scrollLeft -= rowBox.left - tabBox.left;
        else if (tabBox.right > rowBox.right) row.scrollLeft += tabBox.right - rowBox.right;
    }

    /**
     * Swaps in a fresh screen but leaves the shown PDF preview's iframe in the page: an iframe taken out
     * reloads its file and loses the reader's page. Its ancestors stay too, given the fresh ones' attributes
     * and siblings; none of them carries a listener. Any other shape is a plain swap.
     */
    function replaceKeepingPdf(main, fresh) {
        const kept = main.querySelector('iframe.card-preview-pdf');
        const slot = fresh.querySelector('iframe.card-preview-pdf');
        let old = kept;
        let now = slot;
        while (old && now && now !== fresh && old.parentNode !== main) {
            old = old.parentNode;
            now = now.parentNode;
        }
        if (!kept || !slot || kept.getAttribute('src') !== slot.getAttribute('src') || now !== fresh
            || old.parentNode !== main) {
            main.replaceChildren(fresh);
            return;
        }
        for (old = kept, now = slot; now !== fresh; old = old.parentNode, now = now.parentNode) {
            adoptAround(old.parentNode, now.parentNode, old, now);
        }
        Array.from(main.childNodes).forEach(function (child) {
            if (child !== old) child.remove();
        });
    }

    /** Gives `into` the attributes and children of `model`, with `keep` standing where `place` stands. */
    function adoptAround(into, model, keep, place) {
        Array.from(into.attributes).forEach(function (attribute) {
            into.removeAttribute(attribute.name);
        });
        Array.from(model.attributes).forEach(function (attribute) {
            into.setAttribute(attribute.name, attribute.value);
        });
        Array.from(into.childNodes).forEach(function (child) {
            if (child !== keep) child.remove();
        });
        let before = true;
        Array.from(model.childNodes).forEach(function (child) {
            if (child === place) before = false;
            else if (before) into.insertBefore(child, keep);
            else into.appendChild(child);
        });
    }

    function render() {
        // A tip whose anchor is about to be replaced would never see its mouseleave.
        closeTip();
        // Same for the daemon tickers: they write into nodes this render replaces.
        stopAllTickers();
        stopDurationTicker();
        state.screen = currentScreen();
        Array.prototype.forEach.call(document.querySelectorAll('.shell-tab'), function (tab) {
            // A disclosure's own screen sits under the Disclosures tab.
            const screen = state.screen === 'disclosure' ? 'disclosures' : state.screen;
            if (tab.getAttribute('data-screen') === screen) tab.setAttribute('aria-current', 'page');
            else tab.removeAttribute('aria-current');
        });
        revealCurrentTab();
        const identity = document.getElementById('identity');
        if (state.screen === 'access') identity.setAttribute('aria-current', 'page');
        else identity.removeAttribute('aria-current');

        const main = document.getElementById('main');
        document.body.setAttribute('data-screen', state.screen);
        if (state.loading && state.screen !== 'report') {
            main.replaceChildren(el('div', {class: 'card skeleton', style: 'height:220px'}));
            return;
        }
        // Every screen reads limits, workers and the engine version off the status.
        // Without it there is nothing truthful to draw, and reaching for it would
        // throw on the first field.
        if (!state.status) {
            const noGrant = PSL.shellBanner(state.statusError);
            // A caller no grant reaches still reads who they are, to ask an administrator.
            if (noGrant && state.access) main.replaceChildren(renderAccessScreen(noGrant));
            else main.replaceChildren(noGrant ? noGrantBanner(noGrant) : hubUnreachableBanner());
            return;
        }
        if (state.screen === 'access') main.replaceChildren(renderAccessScreen(null));
        else if (state.screen === 'coverage') main.replaceChildren(renderCoverageScreen());
        else if (state.screen === 'sources') main.replaceChildren(renderSourcesScreen());
        else if (state.screen === 'incidents') main.replaceChildren(renderIncidentsScreen());
        else if (state.screen === 'correlations') main.replaceChildren(renderCorrelationsScreen());
        else if (state.screen === 'recap') replaceKeepingPdf(main, renderRecapScreen());
        else if (state.screen === 'disclosures') main.replaceChildren(renderDisclosuresScreen());
        else if (state.screen === 'disclosure') main.replaceChildren(renderDisclosureScreen());
        else if (state.screen === 'ack') main.replaceChildren(renderAckScreen());
        else if (state.screen === 'new') main.replaceChildren(renderNewScreen());
        else if (state.screen === 'run') main.replaceChildren(renderRunScreen(currentRunId()));
        else if (state.screen === 'report') main.replaceChildren(renderReportScreen(currentRunId()));
        else if (state.screen === 'missing') main.replaceChildren(renderMissingScreen());
        else main.replaceChildren(renderRecentScreen());
        startDurationTicker();
        // Here and not only on a route change: a run or a report names itself once it has loaded.
        const heading = document.querySelector('#main h1');
        document.title = heading ? heading.textContent + ' · Perf Sentinel Hub' : 'Perf Sentinel Hub';
    }

    /** A render that would otherwise drop the focus on the body hands it to `selector`. */
    function renderFocus(selector) {
        render();
        const target = document.querySelector(selector);
        if (target) target.focus();
    }

    /**
     * Names the screen in the tab title and, on every route change after the first screen,
     * moves the focus to its heading, as a page load would. A reader of a single-page app
     * otherwise hears nothing when a tab is followed.
     */
    function announceRoute() {
        const heading = document.querySelector('#main h1');
        if (!state.routed) {
            state.routed = true;
            return;
        }
        if (!heading) return;
        heading.setAttribute('tabindex', '-1');
        heading.focus();
    }

    /** The label with a rule running out to its right, as every screen head has. */
    function ruledOverline(text) {
        return el('div', {class: 'overline-ruled'}, [
            el('span', {class: 'overline', text: text}),
            el('span', {class: 'overline-rule', 'aria-hidden': 'true'})
        ]);
    }

    // ------------------------------------------------- shared: help affordance

    /**
     * One floating tip at a time, on <body>. The Sources table scrolls
     * horizontally and clips its own rows, so a tip anchored in CSS cannot
     * escape it. The native title attribute is not an option either: it never
     * appears on keyboard focus.
     */
    let openTip = null;

    function closeTip() {
        if (!openTip) return;
        const anchor = document.querySelector('[aria-describedby="tipbox"]');
        if (anchor) anchor.removeAttribute('aria-describedby');
        openTip.remove();
        openTip = null;
    }

    /**
     * The dashboard's help affordance, same shape and same rule: it sits beside
     * a heading and never inside one, or a screen reader would read the
     * explanation as part of the heading.
     */
    function helpDot(text, topic) {
        // The explanation is the tip's, read as a description while it shows: as
        // the name it made every "?" a paragraph long.
        const dot = el('button', {
            type: 'button', class: 'help-dot', text: '?',
            'aria-label': topic ? t('help.about', {topic: topic.replace(/^\/\/\s*/, '')}) : t('help.label')
        });
        dot.addEventListener('mouseenter', function () {
            showTip(dot, text);
        });
        dot.addEventListener('focus', function () {
            showTip(dot, text);
        });
        dot.addEventListener('mouseleave', closeTip);
        dot.addEventListener('blur', closeTip);
        // Touch has no hover, and the synthesized mouseenter and focus that come
        // before a tap would make a toggle close its own tip. Showing is
        // idempotent, and blur or leaving the dot is what closes.
        dot.addEventListener('click', function () {
            showTip(dot, text);
        });
        return dot;
    }

    function showTip(anchor, text) {
        closeTip();
        const box = proseInto(el('div', {class: 'tipbox', id: 'tipbox', role: 'tooltip'}), text);
        anchor.setAttribute('aria-describedby', 'tipbox');
        document.body.appendChild(box);
        const at = anchor.getBoundingClientRect();
        const size = box.getBoundingClientRect();
        let left = at.right + 10;
        let top = at.top + at.height / 2 - size.height / 2;
        if (left + size.width > globalThis.innerWidth - 12) {
            // No room beside it, so it sits underneath instead.
            left = Math.min(at.left - 16, globalThis.innerWidth - size.width - 12);
            top = at.bottom + 10;
        }
        box.style.left = Math.max(12, left) + 'px';
        box.style.top = Math.max(12, top) + 'px';
        openTip = box;
    }

    /**
     * Text with its backticked spans rendered as code.
     *
     * The daemon writes setting names in backticks, the way its own documentation
     * does. The terminal monitor strips them because a terminal cannot render
     * them, and a browser can. An odd number of backticks is left alone rather
     * than guessed at, so a stray one never turns the rest of a sentence into
     * code.
     */
    function proseInto(parent, text) {
        const parts = String(text).split('`');
        if (parts.length % 2 === 0) {
            parent.appendChild(document.createTextNode(String(text)));
            return parent;
        }
        parts.forEach(function (part, index) {
            if (part === '') return;
            if (index % 2 === 1) parent.appendChild(el('code', {class: 'code-inline', text: part}));
            else parent.appendChild(document.createTextNode(part));
        });
        return parent;
    }

    /** A label with its "?" beside it, which is the only place one belongs. */
    function titledOverline(text, help) {
        return el('div', {class: 'title-row'}, [
            el('h2', {class: 'overline', text: text}),
            help ? helpDot(help, text) : null
        ]);
    }

    // ------------------------------------------------ shared: a copyable command

    /**
     * A command, and one button that copies it.
     *
     * The text arrives already joined: which flags belong on which line is shell
     * syntax rather than a layout choice, so it is decided in PSL beside the
     * quoting.
     */
    /**
     * Every command this product prints runs the engine binary, not the Hub, so
     * it needs one on the machine it is typed into. The link pins the version
     * this Hub runs: that is the one the command above is spelled for.
     */
    function engineNeed(parent) {
        const version = PSL.ENGINE;
        proseInto(parent, version ? t('engine.need', {version: version}) : t('engine.need_unknown'));
        parent.appendChild(el('a', {
            class: 'terminal-link',
            href: PSL.releaseUrl(version),
            // Someone else's site in someone else's tab: the Hub keeps the page the
            // reader was working on, and hands the opener nothing.
            target: '_blank',
            rel: 'noopener noreferrer',
            text: version ? t('engine.release', {version: version}) : t('engine.releases')
        }));
        parent.appendChild(document.createTextNode('.'));
        return parent;
    }

    function engineNote() {
        return engineNeed(el('p', {class: 'terminal-note'}));
    }

    /**
     * The step that trips people up: it names a variable, and a reader who has
     * never met the flag cannot tell what goes in it or where. So it shows the
     * line to run, in the shell they picked, with their own credential to drop
     * in. Only ever shown for a source whose published address takes a header,
     * so an open backend on an intranet never sees any of this.
     */
    function tokenStep(source) {
        const header = source.auth_header_name + ': …';
        const step = el('span', {});
        proseInto(step, t('token.before', {header: source.auth_header_name}));
        const line = el('code', {
            class: 'code-inline step-line',
            text: PSL.exportLine(state.shell, 'PERF_SENTINEL_SOURCE_TOKEN', header)
        });
        SPELL.set(line, function (shellId) {
            return PSL.exportLine(shellId, 'PERF_SENTINEL_SOURCE_TOKEN', header);
        });
        line.setAttribute('data-spell', '');
        step.appendChild(line);
        proseInto(step, t('token.after'));
        return step;
    }

    /** An arrow turning back on itself: put this value where it was. */
    function undoGlyph(size) {
        return svg([
            ['polyline', {points: '3 5 3 11 9 11'}],
            ['path', {d: 'M5.1 15.5a8 8 0 1 0 1.9-8.3L3 11'}]
        ], size);
    }

    /** A circled i, for a block that tells rather than warns. */
    function infoGlyph(size) {
        return svg([
            ['circle', {cx: '12', cy: '12', r: '9'}],
            ['path', {d: 'M12 11.2v5M12 7.8v.2'}]
        ], size);
    }

    /**
     * What has to be true before the command above can run, in the order you
     * would do it. Prose that only explains stays prose below: a numbered step
     * is a promise that there is something to go and do.
     */
    function stepsBlock(steps) {
        const present = steps.filter(Boolean);
        return el('div', {class: 'steps-block'}, [
            el('div', {class: 'steps-head'}, [
                infoGlyph(14),
                el('h2', {class: 'overline', text: t('steps.head')})
            ]),
            // The content is always one node: the row is a two-column grid, and prose
            // carrying a code chip would otherwise arrive as several grid items and
            // be dealt into the columns one piece at a time.
            el('ol', {class: 'steps'}, present.map(function (step) {
                return el('li', {}, [step instanceof Node
                    ? el('span', {class: 'step-text'}, [step])
                    : proseInto(el('span', {class: 'step-text'}), step)]);
            }))
        ]);
    }

    /**
     * One tab per shell, over a single command line. The quoting and the line
     * continuation differ between them, so this is the same request written
     * three ways rather than a preference about how it looks.
     */
    function shellTabs(code, spell) {
        const tabs = PSL.SHELLS.map(function (shell) {
            const tab = el('button', {
                type: 'button',
                class: 'cmd-tab',
                role: 'tab',
                'data-shell': shell.id,
                'aria-selected': shell.id === state.shell ? 'true' : 'false',
                tabindex: shell.id === state.shell ? '0' : '-1',
                text: shell.label
            });
            tab.addEventListener('click', function () {
                choose(shell.id);
            });
            return tab;
        });

        function choose(id) {
            state.shell = id;
            saveShell(id);
            // Every printed command on the page follows: the reader chose a shell,
            // not a tab on one block.
            document.querySelectorAll('[data-spell]').forEach(function (node) {
                const spellFor = SPELL.get(node);
                if (spellFor) node.textContent = spellFor(id);
            });
            document.querySelectorAll('.cmd-tab').forEach(function (node) {
                // By id, not by the label a reader sees: two shells could share a word.
                const selected = node.getAttribute('data-shell') === id;
                node.setAttribute('aria-selected', selected ? 'true' : 'false');
                node.setAttribute('tabindex', selected ? '0' : '-1');
            });
        }

        const strip = el('div', {class: 'cmd-tabs', role: 'tablist', 'aria-label': t('terminal.shells')}, tabs);
        // Arrow keys move between tabs, which is what a tablist promises.
        strip.addEventListener('keydown', function (event) {
            const step = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
            if (!step) return;
            event.preventDefault();
            const index = PSL.SHELLS.findIndex(function (shell) {
                return shell.id === state.shell;
            });
            const next = PSL.SHELLS[(index + step + PSL.SHELLS.length) % PSL.SHELLS.length];
            choose(next.id);
            tabs[PSL.SHELLS.indexOf(next)].focus();
        });
        SPELL.set(code, spell);
        code.setAttribute('data-spell', '');
        return strip;
    }

    // The line each command block would print for a given shell, kept beside the
    // node rather than rebuilt from the form, which has moved on by then.
    const SPELL = new WeakMap();

    function terminalBlock(spec) {
        const spell = spec.spell || null;
        const code = el('pre', {
            class: 'terminal-code',
            tabindex: '0',
            text: spell ? spell(state.shell) : spec.text
        });
        if (spec.id) code.id = spec.id;
        const status = el('span', {class: 'terminal-status', role: 'status'});
        // What this block holds, for the sentence below: the same scaffold renders
        // a command and a configuration file, and neither noun fits both.
        const refused = spec.download ? 'terminal.copy_refused.file' : 'terminal.copy_refused.command';
        const label = el('span', {text: t('terminal.copy')});
        const button = el('button', {
            type: 'button',
            class: 'pill-button terminal-copy',
            'aria-label': spec.copyLabel
        }, [copyGlyph(), label]);

        // One timer per block, held here rather than on state: two blocks are on
        // screen at once, and a shared handle would let one revert cancel the other.
        let timer = 0;
        button.addEventListener('click', function () {
            // Read off the node, not off the spec: a command that changes under the
            // reader would otherwise copy the line it replaced.
            writeClipboard(code).then(function (copied) {
                label.textContent = copied ? t('terminal.copied') : t('terminal.copy');
                if (copied) button.setAttribute('data-copied', 'true');
                status.textContent = copied ? t('terminal.copied_status') : t(refused);
                clearTimeout(timer);
                timer = setTimeout(function () {
                    label.textContent = t('terminal.copy');
                    button.removeAttribute('data-copied');
                    status.textContent = '';
                }, 3200);
            });
        });

        const body = el('div', {class: 'terminal-body'}, [
            spell ? shellTabs(code, spell) : null,
            code,
            el('div', {class: 'terminal-actions'},
                spec.download ? [button, downloadButton(code, spec.download, status), status] : [button, status])
        ]);
        (spec.notes || []).forEach(function (note) {
            if (!note) return;
            body.appendChild(note instanceof Node
                ? note
                : proseInto(el('p', {class: 'terminal-note'}), note));
        });
        if (!spec.fold) {
            return el('section', {class: 'card terminal'}, [
                el('div', {class: 'terminal-head'}, [
                    titledOverline(spec.head, spec.help),
                    el('span', {class: 'terminal-sub', text: spec.sub})
                ]),
                body
            ]);
        }

        // Folded, the overline and its subtitle still say what is behind it: a
        // lone chevron would make the reader open it to find out.
        const toggle = el('button', {
            type: 'button',
            class: 'terminal-more',
            'aria-expanded': spec.fold.open ? 'true' : 'false'
        }, [el('span', {class: 'overline', text: spec.head})]);
        body.hidden = !spec.fold.open;
        toggle.addEventListener('click', function () {
            const next = toggle.getAttribute('aria-expanded') !== 'true';
            toggle.setAttribute('aria-expanded', next ? 'true' : 'false');
            body.hidden = !next;
            spec.fold.onToggle(next);
        });
        return el('section', {class: 'card terminal'}, [
            el('div', {class: 'terminal-head'}, [
                toggle,
                spec.help ? helpDot(spec.help, spec.head) : null,
                el('span', {class: 'terminal-sub', text: spec.sub})
            ]),
            body
        ]);
    }

    /**
     * Hands the block's text over as a file, for a panel whose content is meant
     * to become one. Copying leaves the reader to create the file themselves,
     * which is a step the Hub can take for them.
     *
     * The blob is built at click time from the node, like the copy button, so a
     * fragment that changed under the reader is not the one that lands on disk.
     */
    function downloadButton(code, filename, status) {
        const button = el('button', {
            type: 'button',
            class: 'pill-button terminal-download',
            'aria-label': t('terminal.download_label', {file: filename})
        }, [downloadGlyph(), el('span', {text: t('terminal.download')})]);
        let timer = 0;
        button.addEventListener('click', function () {
            const url = URL.createObjectURL(new Blob([code.textContent], {type: 'application/toml'}));
            // No insertion: a programmatic click on an anchor carrying a download
            // attribute does not need the element to be in the document.
            const link = el('a', {href: url, download: filename});
            link.click();
            // Long enough for the browser to have taken the blob, short enough not to
            // hold one per click for half a minute.
            setTimeout(function () {
                URL.revokeObjectURL(url);
            }, 4000);
            // Said, because a blocked download is otherwise indistinguishable from a
            // click that was never received.
            status.textContent = t('terminal.sent', {file: filename});
            clearTimeout(timer);
            timer = setTimeout(function () {
                status.textContent = '';
            }, 6000);
        });
        return button;
    }

    function downloadGlyph() {
        return svg([
            ['path', {d: 'M12 4v10M8 11l4 4 4-4'}],
            ['path', {d: 'M5 19h14'}]
        ], 14);
    }

    /**
     * Copies, or selects and says so.
     *
     * navigator.clipboard exists only in a secure context and this Hub is
     * routinely served over plain HTTP on an internal address. The check comes
     * before the call rather than after a rejection: Firefox exposes the object
     * in an insecure context and rejects, and by the time that promise settles
     * the user activation execCommand needs is gone.
     *
     * When even that is refused, the selection it made is left in place: the
     * operator finishes with their own keystroke, and the block says so instead
     * of pretending the click worked.
     */
    function writeClipboard(code) {
        const text = code.textContent;
        if (globalThis.isSecureContext && navigator.clipboard && navigator.clipboard.writeText) {
            // The label waits for this promise: claiming "Copied" before it settles
            // would announce a success a denied permission can still take back.
            return navigator.clipboard.writeText(text).then(
                function () {
                    return true;
                },
                function () {
                    selectNode(code);
                    return false;
                });
        }
        const selection = selectNode(code);
        let copied = false;
        try {
            copied = document.execCommand('copy');
        } catch (error) {
            copied = false;
        }
        if (copied && selection) selection.removeAllRanges();
        return Promise.resolve(copied);
    }

    function selectNode(node) {
        const range = document.createRange();
        range.selectNodeContents(node);
        const selection = getSelection();
        if (!selection) return null;
        selection.removeAllRanges();
        selection.addRange(range);
        return selection;
    }

    function copyGlyph() {
        return svg([
            ['rect', {x: '9', y: '9', width: '11', height: '11', rx: '2'}],
            ['path', {d: 'M5 15V5a2 2 0 0 1 2-2h10'}]
        ], 13);
    }

    /** Loads whatever the route needs, then renders it. */
    function onRoute() {
        // First, before the render and before the loading guard: loadShell's own
        // onRoute has to apply it again, and loadRuns never re-renders New.
        applyHandoff(PSL.readHandoff(location.hash, Date.now()));
        applyTraceHandoff(PSL.readTraceHandoff(location.hash));
        const screen = currentScreen();
        if (screen !== 'ack') state.ackReturn = null;
        clearTimeout(state.runTimer);
        // The note this timer restores belongs to a panel the next render replaces.
        clearTimeout(state.noteTimer);
        render();
        if (state.loading) return;
        announceRoute();
        // Both screens read the runs list: recent renders it, the launcher
        // shows what past runs weighed. Reloaded on every entry, not once, so
        // coming back from a run that just finished shows its weight.
        if (screen === 'recent' || screen === 'new') loadRuns();
        else if (screen === 'incidents') loadIncidents();
        else if (screen === 'correlations') {
            // A link to one pair, from Grafana or an incident, reads it over the widest range.
            state.correlationFilter.pair_key = PSL.readCorrelationPair(location.hash);
            if (state.correlationFilter.pair_key) state.correlationFilter.range = PSL.CORRELATION_RANGES[PSL.CORRELATION_RANGES.length - 1];
            loadCorrelations();
        } else if (screen === 'recap') loadRecap(false);
        else if (screen === 'disclosures') loadDisclosures();
        else if (screen === 'disclosure') {
            const id = PSL.readDisclosureId(location.hash);
            // A note and a token belong to the disclosure they were made on.
            if (!state.disclosure || !id || state.disclosure.disclosure.id !== id) {
                state.disclosureNote = null;
                state.disclosureLink = null;
                state.withdrawOpen = false;
            }
            if (id) loadDisclosure(id);
            else state.disclosureError = true;
        } else if (screen === 'ack') loadAck(false);
        else if (screen === 'run' || screen === 'report') {
            const id = currentRunId();
            if (id && (!state.run || state.run.id !== id)) {
                state.runReadFailures = 0;
                loadRun(id);
            } else if (id) {
                // The render above already mounted the report frame: a second one would load it twice.
                if (screen === 'run') render();
                // The report frame cannot say its session lapsed: a 401 there is
                // a blank pane. Asking the API is what reloads into the sign-in.
                // No render on success, which would reload the report again.
                if (screen === 'report') getJson('/api/analyses/' + id).catch(function () {
                });
            }
        }
    }

    /**
     * Pre-fills the form from an incident link: its service, its window as an
     * absolute range. The row it names is added when the incidents screen still
     * holds it, for the banner's wording. Anything else on the form stays.
     */
    function applyHandoff(handoff) {
        // A route without the query does not end the handoff: the tab points at
        // the bare `#/new`, and dropping the banner there would leave the form
        // holding a window the operator never typed with nothing explaining it.
        // The banner goes when the form stops holding that window.
        if (!handoff) {
            const kept = state.handoff;
            const intact = kept && state.form.rangeMode === 'absolute' && state.form.fromMs === kept.fromMs
                && state.form.toMs === kept.toMs && state.form.service === kept.service;
            if (!intact) state.handoff = null;
            return;
        }
        state.handoff = handoff;
        state.form.mode = 'service';
        state.form.service = handoff.service;
        state.form.traceId = '';
        state.form.rangeMode = 'absolute';
        state.form.fromMs = handoff.fromMs;
        state.form.toMs = handoff.toMs;
        state.form.pickerOpen = false;
        const row = (state.incidents || []).find(function (incident) {
            return incident.id === handoff.incidentId;
        });
        if (!row) return;
        handoff.kind = row.kind;
        handoff.namespace = row.namespace;
        handoff.atMs = row.at_ms;
    }

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
        // A trace replaces a window handed over before it, banner included.
        state.handoff = null;
        state.form.mode = 'trace';
        state.form.traceId = handoff.traceId;
        state.form.service = '';
        state.form.pickerOpen = false;
    }

    // -------------------------------------------------------- screen: sources

    function renderSourcesScreen() {
        const section = el('section', {}, [
            ruledOverline(t('sources.overline')),
            el('h1', {class: 'page-title', text: t('sources.title')}),
            el('p', {class: 'page-sub', text: t('sources.sub')})
        ]);

        if (state.loading) {
            section.appendChild(el('div', {class: 'sources-wrap'}, [skeletonTable()]));
            return section;
        }
        if (state.sourcesError) {
            // Showing the last known values here would be worse than showing none:
            // a stale health table is the one thing this page must never be.
            section.appendChild(el('div', {class: 'banner', 'data-tone': 'crit'}, [
                critGlyph(16),
                el('div', {text: t('sources.hub_down')})
            ]));
            return section;
        }

        section.appendChild(el('div', {class: 'sources-wrap'}, [sourcesTable(state.sources)]));
        return section;
    }

    const SOURCE_COLUMNS = ['source', 'type', 'env', 'health', 'last_success', 'down_for', 'producer', 'last_error'];
    // The columns whose cells are right-aligned. The headings have to follow, or
    // a value sits under the gap beside its own label.
    const SOURCE_COLUMNS_RIGHT = ['last_success', 'down_for', 'producer'];

    function sourcesTable(sources) {
        const head = el('tr', {}, SOURCE_COLUMNS.map(function (name) {
            return el('th', {
                text: t('sources.col.' + name),
                scope: 'col',
                'data-align': SOURCE_COLUMNS_RIGHT.indexOf(name) >= 0 ? 'right' : null
            });
        }));
        // PSL.splitByKind keeps each source's original position, which the fold ids
        // are built from, so a grouped row still addresses the source it belongs to.
        const kinds = PSL.splitByKind(sources);
        const rows = function (group) {
            return group.flatMap(function (entry) {
                return sourceRow(entry.source, entry.index);
            });
        };
        const body = kinds.split
            ? [tableGroupRow(t('sources.group.daemons'))].concat(rows(kinds.daemons),
                [tableGroupRow(t('sources.group.backends'))],
                rows(kinds.backends))
            : sources.flatMap(sourceRow);
        return el('table', {class: 'table'}, [
            el('thead', {}, [head]),
            el('tbody', {}, body)
        ]);
    }

    function tableGroupRow(text) {
        return el('tr', {class: 'table-group'}, [
            el('td', {colspan: String(SOURCE_COLUMNS.length)}, [el('span', {class: 'overline', text: text})])
        ]);
    }

    function sourceRow(source, index) {
        const now = Date.now();
        const row = el('tr', source.reachable ? {} : {'data-unreachable': 'true'});
        // Only a daemon has something to unfold, so only a daemon gets a control, and
        // with access on only where the caller holds sources.read, which its view needs.
        const unfolds = source.kind === 'daemon' && PSL.permits(source, 'sources.read');
        if (unfolds) row.appendChild(daemonNameCell(source, index));
        else if (source.kind === 'daemon') {
            // Said, not left as a row that simply stops opening.
            row.appendChild(el('td', {class: 'table-strong'}, [
                el('span', {text: source.name}),
                el('span', {class: 'source-locked', text: t('sources.locked')})
            ]));
        } else row.appendChild(el('td', {class: 'table-strong', text: source.name}));
        row.appendChild(el('td', {}, [el('span', {class: 'chip', text: PSL.KIND_LABEL[source.kind] || source.kind})]));
        row.appendChild(el('td', {}, [el('span', {class: 'chip chip-declared', text: source.environment})]));
        row.appendChild(el('td', {}, [healthCell(source, now)]));
        // Ages, so they tick with the panel beside them instead of freezing at the render.
        const lastSuccess = function () {
            return source.last_success_ms
                ? t('sources.ago', {duration: PSL.dur(Date.now() - source.last_success_ms)})
                : t('sources.never');
        };
        const downFor = function () {
            return source.unreachable_since_ms ? PSL.dur(Date.now() - source.unreachable_since_ms) : '—';
        };
        row.appendChild(live(el('td', {'data-align': 'right', text: lastSuccess()}), lastSuccess));
        row.appendChild(live(el('td', {'data-align': 'right', text: downFor()}), downFor));
        row.appendChild(producerCell(source));
        row.appendChild(el('td', {class: 'table-mono', text: source.last_error_code || '—'}));
        if (!unfolds) return [row];

        // Spanning the header count and not the literal 8: adding a column must
        // not be able to break the detail row.
        const cell = el('td', {
            id: 'daemon-detail-' + index,
            colspan: String(SOURCE_COLUMNS.length)
        }, [daemonPanel(source, index)]);
        const detail = el('tr', {class: 'daemon-detail'}, [cell]);
        detail.hidden = state.daemonOpen[source.id] !== true;
        // A render stopped every ticker, so a row rebuilt open re-arms its own
        // once the table is attached, and one opened by a link from another
        // screen runs its first read here.
        if (state.daemonOpen[source.id] === true) {
            queueMicrotask(function () {
                const view = state.daemonViews[source.id];
                if (view === undefined) loadDaemon(source, index);
                else if (view !== 'loading' && !view.error_code) startTicker(source, index);
            });
        }
        return [row, detail];
    }

    function healthCell(source, now) {
        const health = PSL.sourceHealth(source, now);
        return el('span', {class: 'health', 'data-health': health.tone}, [
            el('span', {class: 'health-dot'}),
            el('span', {text: health.text})
        ]);
    }

    function producerCell(source) {
        if (!source.producer_version) {
            // Two different absences. A backend has no producer at all, and saying
            // so about a daemon nobody has reached yet would be a false statement
            // about a source that does have one.
            return source.kind === 'daemon'
                ? el('td', {
                    class: 'table-muted',
                    'data-align': 'right',
                    text: t('sources.producer.unknown'),
                    title: t('sources.producer.unknown_title')
                })
                : el('td', {
                    class: 'table-muted',
                    'data-align': 'right',
                    text: t('unit.na'),
                    title: t('sources.producer.none_title')
                });
        }

        const cell = el('td', {
            class: 'table-mono',
            'data-align': 'right'
        }, [el('span', {text: source.producer_version})]);
        const gap = PSL.skew(source.producer_version);
        if (gap) {
            cell.appendChild(el('span', {
                class: 'skew-pill',
                'data-dir': gap.dir,
                text: gap.label,
                title: t('sources.skew_title')
            }));
        }
        return cell;
    }

    function skeletonTable() {
        const rows = [];
        for (let index = 0; index < 4; index++) rows.push(el('div', {class: 'skeleton skeleton-row'}));
        return el('div', {class: 'skeleton-stack'}, rows);
    }


    // ------------------------------------------------- sources: the daemon row

    /**
     * The [daemon] section in the order the daemon's own monitor prints it, so
     * the two surfaces never contradict each other. Environment stands alone:
     * it is the one value that claims something about the world rather than
     * about the process.
     */
    const DAEMON_GROUPS = [
        ['declared', ['environment']],
        ['ingestion', ['sampling_rate', 'max_active_traces', 'trace_ttl_ms', 'max_events_per_trace',
            'max_payload_size', 'ingest_queue_capacity', 'analysis_queue_capacity', 'memory_high_water_pct']],
        ['readers', ['max_retained_findings', 'max_export_findings', 'max_retained_traces']],
        ['labels', ['per_service_labels', 'per_grouping_labels']],
        ['listeners', ['api_enabled', 'listen_addr', 'listen_port', 'listen_port_grpc', 'json_socket']],
        ['subsystems', ['tls_configured', 'ack_enabled', 'ack_api_key_set', 'cors_allowed_origins',
            'archive_configured']],
        ['correlation', ['correlation_enabled', 'correlation_window_ms', 'correlation_lag_threshold_ms',
            'correlation_min_co_occurrences', 'correlation_min_confidence', 'correlation_max_tracked_pairs']]
    ];

    /** The text key of what each setting costs when it is wrong. Two say enough by their name. */
    const DAEMON_COPY = {
        environment: 'daemon.setting.environment',
        sampling_rate: 'daemon.setting.sampling_rate',
        max_active_traces: 'daemon.setting.max_active_traces',
        trace_ttl_ms: 'daemon.setting.trace_ttl_ms',
        max_events_per_trace: 'daemon.setting.max_events_per_trace',
        max_payload_size: 'daemon.setting.max_payload_size',
        ingest_queue_capacity: 'daemon.setting.ingest_queue_capacity',
        analysis_queue_capacity: 'daemon.setting.analysis_queue_capacity',
        memory_high_water_pct: 'daemon.setting.memory_high_water_pct',
        max_retained_findings: 'daemon.setting.max_retained_findings',
        max_export_findings: 'daemon.setting.max_export_findings',
        max_retained_traces: 'daemon.setting.max_retained_traces',
        per_service_labels: 'daemon.setting.per_service_labels',
        per_grouping_labels: 'daemon.setting.per_grouping_labels',
        api_enabled: 'daemon.setting.api_enabled',
        listen_addr: 'daemon.setting.listen_addr',
        json_socket: 'daemon.setting.json_socket',
        tls_configured: 'daemon.setting.tls_configured',
        ack_enabled: 'daemon.setting.ack_enabled',
        ack_api_key_set: 'daemon.setting.ack_api_key_set',
        cors_allowed_origins: 'daemon.setting.cors_allowed_origins',
        archive_configured: 'daemon.setting.archive_configured',
        correlation_enabled: 'daemon.setting.correlation_enabled',
        correlation_window_ms: 'daemon.setting.correlation_window_ms',
        correlation_lag_threshold_ms: 'daemon.setting.correlation_lag_threshold_ms',
        correlation_min_co_occurrences: 'daemon.setting.correlation_min_co_occurrences',
        correlation_min_confidence: 'daemon.setting.correlation_min_confidence',
        correlation_max_tracked_pairs: 'daemon.setting.correlation_max_tracked_pairs',
        energy_source: 'daemon.setting.energy_source',
        api_version: 'daemon.setting.api_version',
        emission_factor_type: 'daemon.setting.emission_factor_type',
        temporal_granularity: 'daemon.setting.temporal_granularity',
        electricity_maps: 'daemon.setting.electricity_maps',
        grid_feeds: 'daemon.setting.grid_feeds',
        fingrid_dataset: 'daemon.setting.fingrid_dataset',
        per_operation_coefficients: 'daemon.setting.per_operation_coefficients',
        use_hourly_profiles: 'daemon.setting.use_hourly_profiles',
        embodied_per_request_gco2: 'daemon.setting.embodied_per_request_gco2',
        network_energy_per_byte_kwh: 'daemon.setting.network_energy_per_byte_kwh'
    };

    /**
     * The export serialises four thresholds under different names than the file
     * keys the launcher's knobs carry. The same settings seen from the other
     * side, so they take the same sentences rather than a second copy of them.
     */
    const DETECT_ALIAS = {
        n_plus_one_threshold: 'n_plus_one_min_occurrences',
        window_ms: 'window_duration_ms',
        slow_threshold_ms: 'slow_query_threshold_ms',
        slow_min_occurrences: 'slow_query_min_occurrences'
    };

    /** The sentence for one setting, or none. */
    function settingCopy(name) {
        const key = DAEMON_COPY[name] || DETECTION_COPY[DETECT_ALIAS[name] || name];
        return key ? t(key) : null;
    }

    /**
     * Mirrors the daemon monitor's own colouring, so the two surfaces agree. An
     * unknown kind stays muted rather than dressed as an alarm: a kind this Hub
     * predates is not automatically bad news.
     */
    const DAEMON_HINT_TONE = {
        ingestion_drops: 'crit',
        tuning: 'warn',
        cold_start: 'muted',
        snapshot_scope: 'info'
    };

    /**
     * A real button and not a clickable row: Enter, Space, the focus ring and the
     * semantics come free, and text stays selectable.
     */
    function daemonNameCell(source, index) {
        const button = el('button', {
            type: 'button',
            class: 'row-toggle',
            'aria-expanded': state.daemonOpen[source.id] === true ? 'true' : 'false',
            'aria-controls': 'daemon-detail-' + index
        }, [el('span', {text: source.name})]);
        button.addEventListener('click', function () {
            toggleDaemon(source, button, index);
        });
        return el('td', {class: 'table-strong'}, [button]);
    }

    /**
     * Folded in place rather than through render(), which would rebuild the
     * wrapper and reset its horizontal scroll. Fetched once and kept: a settings
     * snapshot is not a live figure, and refetching on every fold would hit the
     * daemon for a fold rather than for a question.
     */
    function toggleDaemon(source, button, index) {
        const open = button.getAttribute('aria-expanded') !== 'true';
        button.setAttribute('aria-expanded', open ? 'true' : 'false');
        state.daemonOpen[source.id] = open;
        saveFolds();
        const cell = document.getElementById('daemon-detail-' + index);
        if (!cell) return;
        cell.parentNode.hidden = !open;
        // A folded row is not being read, so it stops costing the daemon anything.
        if (!open) {
            stopTicker(source.id);
            return;
        }
        const view = state.daemonViews[source.id];
        if (view === 'loading') return;
        // An error is not kept as an answer. The row re-reads itself on its
        // interval, and reopening the fold asks again straight away.
        if (view !== undefined && !view.error_code) {
            startTicker(source, index);
            return;
        }
        loadDaemon(source, index);
    }

    /** The first read of a daemon row, and the retry after a failed one. */
    function loadDaemon(source, index) {
        state.daemonViews[source.id] = 'loading';
        const cell = document.getElementById('daemon-detail-' + index);
        if (cell) cell.replaceChildren(daemonPanel(source, index));
        getJson('/api/sources/' + encodeURIComponent(source.id) + '/daemon')
            .then(function (view) {
                state.daemonViews[source.id] = view;
                state.daemonReadAt[source.id] = Date.now();
                state.daemonFullReadAt[source.id] = Date.now();
            })
            .catch(function (error) {
                // The gate's 503 is the Hub being briefly full, not the Hub failing:
                // it clears in about a second and deserves its own sentence.
                state.daemonViews[source.id] = {
                    error_code: /answered 503$/.test(String(error && error.message)) ? 'hub_busy' : 'internal'
                };
            })
            .finally(function () {
                // The table may have been rebuilt while this was in flight, so the
                // cell is found again rather than kept in a closure.
                const target = document.getElementById('daemon-detail-' + index);
                if (target) target.replaceChildren(daemonPanel(source, index));
                // And the row may have been folded in the meantime, in which case
                // starting to poll it would contradict the fold. A failed read still
                // gets a ticker: its panel carries the same countdown, and the row
                // recovers on its own rather than waiting to be refolded.
                if (state.daemonOpen[source.id]) startTicker(source, index);
            });
    }

    function daemonPanel(source, index) {
        const view = state.daemonViews[source.id];
        if (view === 'loading' || view === undefined) {
            return el('div', {class: 'daemon-panel'}, [
                el('p', {class: 'daemon-loading', role: 'status', text: t('daemon.reading', {name: source.name})}),
                el('div', {class: 'skeleton', style: 'height:150px'})
            ]);
        }
        if (view.error_code) return daemonError(source, index, view.error_code);

        return el('div', {class: 'daemon-panel'}, [
            el('p', {class: 'overline daemon-audience', text: t('daemon.audience')}),
            el('p', {class: 'daemon-source-note', text: t('daemon.source_note')}),
            el('div', {id: 'daemon-top-' + index}, [daemonTopRow(source, view, index)]),
            terminalBlock({
                head: t('daemon.terminal.head'),
                sub: t('daemon.terminal.sub'),
                id: 'monitor-command-' + index,
                spell: function (shellId) {
                    return PSL.monitorCommand(source, refreshSeconds(source.id), shellId);
                },
                // Folded by default: the row is opened to read the gauges, and this is
                // the other way of reading them rather than part of that answer.
                fold: {
                    open: state.daemonTerminalOpen[source.id] === true,
                    onToggle: function (open) {
                        state.daemonTerminalOpen[source.id] = open;
                        saveFolds();
                    }
                },
                copyLabel: t('daemon.terminal.copy', {name: source.name}),
                notes: [
                    t('daemon.terminal.note'),
                    engineNote(),
                    PSL.monitorKeyHint(source)
                ]
            }),
            settingsDisclosure(source, view)
        ]);
    }

    /**
     * A failed read is not a dead end: the row keeps asking on the same interval
     * the healthy rows use, and the first answer replaces this with the daemon.
     */
    function daemonError(source, index, code) {
        return el('div', {class: 'daemon-panel'}, [
            el('div', {class: 'banner', 'data-tone': 'crit'}, [
                critGlyph(16),
                el('div', {}, [
                    el('p', {}, [
                        el('span', {text: t('daemon.error.returned')}),
                        el('span', {class: 'code-inline', text: code}),
                        proseInto(el('span'), t('daemon.error.meaning', {
                            meaning: PSL.READ_ERRORS[code] || PSL.ERRORS[code] || t('daemon.error.unreached')
                        }))
                    ]),
                    el('p', {class: 'notice-sub', text: t('daemon.error.sub')})
                ])
            ]),
            el('p', {class: 'daemon-lead', text: t('daemon.error.retry')}),
            refreshControl(source, index)
        ]);
    }

    /** The tone of each verdict, its words under `daemon.verdict.<state>`. */
    const DAEMON_VERDICT = {
        ok: 'ok',
        near_capacity: 'warn',
        advised: 'warn',
        unknown: 'muted'
    };

    /**
     * What the fold opens on: the verdict, the gauges, and the daemon's own
     * hints beside them rather than under them. The settings are the long part
     * and stay behind one more click.
     */
    function pct(gauge) {
        return gauge ? gauge.pct : null;
    }

    function daemonTopRow(source, view, index) {
        const verdict = DAEMON_VERDICT[view.state] ? view.state : 'unknown';
        // Read once and cleared: the badges belong to the read that produced them,
        // not to every rebuild that happens to come after it.
        const moves = state.daemonMoves[source.id] || {};
        delete state.daemonMoves[source.id];
        const main = el('div', {class: 'daemon-top-main'}, [
            el('div', {class: 'sink-head'}, [
                titledOverline(t('daemon.now.head'), t('daemon.now.help')),
                el('span', {class: 'sink-sub refresh-read', id: 'refresh-read-' + index})
            ]),
            refreshControl(source, index),
            countStrip([
                [gaugeText(view.traces), t('daemon.gauge.traces'), PSL.gaugeTone(pct(view.traces)), moves.traces],
                [gaugeText(view.analysis_queue), t('daemon.gauge.queue'),
                    PSL.gaugeTone(pct(view.analysis_queue)), moves.analysis_queue],
                // No tone: the ring fills on any daemon that runs long enough, and
                // the Hub keeps what it evicts. A poll that missed some says so in the hints.
                [gaugeText(view.findings), t('daemon.gauge.findings'), null, moves.findings],
                // No cap and only one direction: an uptime that grows every read is
                // not news, and a tone would say it is running out of something. Down to
                // the minute, because two units hide a whole day: a daemon up for 10 d
                // 23 h reads the same as one up for 10 d flat.
                [view.uptime_seconds == null ? t('daemon.unknown') : PSL.durMinutes(view.uptime_seconds * 1000),
                    t('daemon.gauge.uptime')]
            ]),
            el('p', {class: 'daemon-lead', text: t('daemon.now.lead')})
        ]);

        const side = el('div', {class: 'daemon-top-side'}, [
            el('div', {class: 'sink-head'}, [
                titledOverline(t('daemon.hints.head'), t('daemon.hints.help')),
                el('span', {class: 'sink-sub', text: t('daemon.hints.sub')})
            ])
        ]);
        if (view.hints_unavailable_reason) {
            // An unread export is not a clean bill: silence has to be earned.
            side.appendChild(proseInto(el('p', {class: 'daemon-lead'}),
                t('daemon.hints.unread', {reason: view.hints_unavailable_reason})));
        } else if (view.warnings.length === 0) {
            side.appendChild(el('p', {class: 'daemon-lead', text: t('daemon.hints.none')}));
        } else {
            view.warnings.forEach(function (hint) {
                side.appendChild(el('div', {class: 'outcome-warning'}, [
                    el('span', {
                        class: 'outcome-warning-kind',
                        'data-tone': DAEMON_HINT_TONE[hint.kind] || 'muted',
                        text: hint.kind
                    }),
                    // Written by the daemon, so it stays in the daemon's English.
                    engineText(proseInto(el('span', {class: 'outcome-warning-message'}), hint.message))
                ]));
            });
            if (view.warnings_dropped > 0) {
                side.appendChild(el('p', {
                    class: 'daemon-lead',
                    text: t(view.warnings_dropped === 1 ? 'daemon.hints.dropped.one' : 'daemon.hints.dropped.many',
                        {count: view.warnings_dropped})
                }));
            }
        }

        return el('div', {}, [
            el('div', {class: 'daemon-verdict-row'}, [
                el('span', {
                    class: 'daemon-verdict',
                    'data-tone': DAEMON_VERDICT[verdict],
                    text: t('daemon.verdict.' + verdict)
                })
            ]),
            el('div', {class: 'daemon-top'}, [main, side])
        ]);
    }

    /**
     * How often this row re-reads the daemon, and how long until it does.
     *
     * The same knob `query monitor --refresh` carries, with an off position the
     * command does not have: a terminal session is opened to watch, a table row
     * is often opened just to read a setting once.
     */
        // Declared in seconds, not milliseconds: the engine's `--refresh` takes whole
        // seconds and the printed command mirrors this list, so a choice that is not
        // one would drop the flag while the row kept re-reading. Deriving the
        // milliseconds from the seconds makes that impossible rather than unlikely.
    const REFRESH_SECONDS = [0, 5, 10, 30, 60];
    const DEFAULT_REFRESH_MS = 5000;

    function refreshMs(sourceId) {
        const chosen = state.daemonRefreshMs[sourceId];
        return chosen === undefined ? DEFAULT_REFRESH_MS : chosen;
    }

    function refreshSeconds(sourceId) {
        return refreshMs(sourceId) / 1000;
    }

    function refreshControl(source, index) {
        const ms = refreshMs(source.id);
        const select = el('select', {class: 'refresh-select', 'aria-label': t('daemon.refresh.label')});
        REFRESH_SECONDS.forEach(function (seconds) {
            const option = el('option', {
                value: String(seconds * 1000),
                text: seconds ? seconds + ' ' + t('unit.s') : t('daemon.refresh.off')
            });
            if (seconds * 1000 === ms) option.selected = true;
            select.appendChild(option);
        });
        // Which device put the focus there, for the ring rule in the stylesheet.
        select.addEventListener('pointerdown', function () {
            select.dataset.pointer = 'true';
        });
        select.addEventListener('keydown', function () {
            delete select.dataset.pointer;
        });
        select.addEventListener('blur', function () {
            delete select.dataset.pointer;
        });
        select.addEventListener('change', function () {
            state.daemonRefreshMs[source.id] = Number(select.value);
            startTicker(source, index);
            // Only the line is rewritten: the note under it is worded to hold at any
            // interval, including off, so it never needs to be.
            const printed = document.getElementById('monitor-command-' + index);
            if (printed) {
                printed.textContent = PSL.monitorCommand(source, refreshSeconds(source.id), state.shell);
            }
        });
        const ring = refreshRing();
        if (ms) ring.querySelector('.refresh-ring-fill').style.setProperty('--cycle', ms + 'ms');

        // Its own line under the heading, not squeezed beside it. The ring is
        // decoration, the sentence is the information: under reduced motion the
        // ring stops moving and the countdown still counts.
        return el('div', {class: 'refresh'}, [
            ring,
            el('span', {class: 'refresh-next', id: 'refresh-next-' + index, role: 'status'}),
            el('span', {class: 'refresh-label', text: t('daemon.refresh.every')}),
            select
        ]);
    }

    /**
     * The cycle as a filling disc. It sweeps once per interval and carries on
     * into the next one: the sweep is timed from the cycle's own start rather
     * than from the last read, so the network time of a read does not show up as
     * a pause at full.
     */
    function refreshRing() {
        const node = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
        node.setAttribute('viewBox', '0 0 24 24');
        node.setAttribute('width', '14');
        node.setAttribute('height', '14');
        node.setAttribute('aria-hidden', 'true');
        node.setAttribute('class', 'refresh-ring');

        const track = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
        track.setAttribute('cx', '12');
        track.setAttribute('cy', '12');
        track.setAttribute('r', '11');
        track.setAttribute('class', 'refresh-ring-track');
        node.appendChild(track);

        // A solid disc drawn as one stroked circle: at half the radius with a
        // stroke as thick as the diameter, the stroke covers the whole disc, so
        // the dash sweeps a filled wedge rather than an outline. r 5.25 puts the
        // wedge's outer edge exactly on the track's inner edge instead of over
        // it, and pathLength lets the stylesheet count in percent.
        const fill = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
        fill.setAttribute('cx', '12');
        fill.setAttribute('cy', '12');
        fill.setAttribute('r', '5.25');
        fill.setAttribute('pathLength', '100');
        fill.setAttribute('class', 'refresh-ring-fill');
        node.appendChild(fill);
        return node;
    }

    /**
     * One second, one job: it writes the countdown and fires the read when the
     * time is up. Deriving the deadline from the last read rather than counting
     * down a variable means a slow tab cannot make the interval drift.
     */
    function startTicker(source, index) {
        stopTicker(source.id);
        restartCycle(source, index);
        tickRefresh(source, index);
        if (!refreshMs(source.id)) return;
        state.daemonTickers[source.id] = setInterval(function () {
            tickRefresh(source, index);
        }, 1000);
    }

    /**
     * The sweep is one looping CSS animation whose duration is the interval, not
     * a value written every second: a per-second write can only land on whole
     * seconds, so the last slice of the disc never got drawn before the cycle
     * turned over. Restarted only to resynchronise it with a read.
     */
    function restartSweep(index, ms) {
        const host = document.getElementById('refresh-next-' + index);
        const fill = host && host.parentNode.querySelector('.refresh-ring-fill');
        if (!fill) return;
        if (!ms) {
            fill.style.animation = 'none';
            return;
        }
        fill.style.animation = 'none';
        fill.style.setProperty('--cycle', ms + 'ms');
        void fill.getBoundingClientRect();
        fill.style.animation = '';
    }

    /** The disc and the countdown clock restart together, from one place. */
    function restartCycle(source, index) {
        const ms = refreshMs(source.id);
        state.daemonCycleAt[source.id] = Date.now();
        restartSweep(index, ms);
        // The disc and the read now share one deadline instead of one starting the
        // other: the read lands when the sweep closes, not on the beat after it.
        clearTimeout(state.daemonCycleTimers[source.id]);
        if (!ms) return;
        state.daemonCycleTimers[source.id] = setTimeout(function () {
            refreshDaemon(source, index);
        }, ms);
    }

    function stopTicker(sourceId) {
        clearInterval(state.daemonTickers[sourceId]);
        delete state.daemonTickers[sourceId];
        clearTimeout(state.daemonCycleTimers[sourceId]);
        delete state.daemonCycleTimers[sourceId];
    }

    function stopAllTickers() {
        // Both maps: a source can hold a cycle timer without holding a ticker, and
        // walking only the tickers would leave that one running past the screen.
        new Set(Object.keys(state.daemonTickers).concat(Object.keys(state.daemonCycleTimers)))
            .forEach(stopTicker);
    }

    function tickRefresh(source, index) {
        const read = document.getElementById('refresh-read-' + index);
        const next = document.getElementById('refresh-next-' + index);
        // The countdown is the ticker's only requirement. A failed row carries one
        // and no read age, having never had a read to age.
        if (!next) {
            stopTicker(source.id);
            return;
        }

        if (read) {
            const readAt = state.daemonReadAt[source.id] || Date.now();
            read.textContent = t('daemon.refresh.read', {duration: PSL.dur(Date.now() - readAt)});
        }

        const ms = refreshMs(source.id);
        // The sweep runs on its own clock, set by startTicker and refreshDaemon.
        // Timing it from the last read would hold the disc at full for however
        // long the request takes, which reads as a stall rather than as a cycle.
        const cycleAt = state.daemonCycleAt[source.id];
        const ring = next.parentNode.querySelector('.refresh-ring');
        const fill = ring && ring.querySelector('.refresh-ring-fill');
        if (!ms) {
            next.textContent = t('daemon.refresh.stopped');
            if (ring) ring.hidden = true;
            return;
        }
        if (ring) ring.hidden = false;
        const left = Math.max(0, cycleAt + ms - Date.now());
        next.textContent = t('daemon.refresh.next', {seconds: Math.ceil(left / 1000)});
    }

    /**
     * Re-reads and replaces the state block only. A daemon's settings do not
     * change without a restart, so rebuilding those every few seconds would
     * throw away the reader's open groups and their focus for nothing.
     */
        // A light tick costs the daemon one status read. The full read, which
        // buffers the export to refresh the hints, runs at most this often.
    const FULL_READ_EVERY_MS = 60000;

    /**
     * Swaps a rebuilt block in, unless the reader is inside it: rebuilding would
     * close the interval select under their pointer. The next tick catches up.
     *
     * @param {string} id the block to replace
     * @param {() => Node} build called only when the swap is going ahead, since
     *   building a row has side effects the discarded copy would swallow
     * @returns {boolean} whether the swap happened
     */
    function replaceIfIdle(id, build) {
        const host = document.getElementById(id);
        if (!host || host.contains(document.activeElement)) return false;
        // Built here rather than by the caller: daemonTopRow consumes the move
        // badges as it renders them, and building a row that is then discarded
        // would spend that read's badges on nothing.
        host.replaceChildren(build());
        return true;
    }

    function refreshDaemon(source, index) {
        // A folded row is not being read. The cycle timer can fire in the very gap
        // between the fold clearing it and this call arming the next one, and that
        // one would then re-arm itself for good, on a row nobody is looking at.
        if (state.daemonOpen[source.id] !== true) return;
        // A separate flag rather than a sentinel in daemonViews: a render during
        // the flight reads daemonViews, and a string there would crash it.
        if (state.daemonInFlight[source.id]) {
            // Slower than its own interval. Wait a whole one rather than queue up
            // behind it, and keep the cycle armed so the row does not go quiet.
            restartCycle(source, index);
            return;
        }
        state.daemonInFlight[source.id] = true;
        restartCycle(source, index);
        const previous = state.daemonViews[source.id];
        // A light read is only ever an overlay on a full one, so a row with
        // nothing to lay it over asks the cheap question first, and only reads in
        // full once it knows there is something to read.
        const plan = PSL.refreshPlan(
            previous,
            Date.now() - (state.daemonFullReadAt[source.id] || 0),
            FULL_READ_EVERY_MS);
        // Read from the plan rather than judged a second time: the plan only says
        // "light" for a view that can be merged onto, so the two cannot disagree.
        const kept = plan === 'light' ? previous : null;
        getJson('/api/sources/' + encodeURIComponent(source.id) + '/daemon'
            + (plan === 'full' ? '' : '?refresh=status'))
            .then(function (view) {
                if (plan === 'probe') {
                    // The daemon answers again: only a full read renders it, and that is
                    // the same read this row started with, skeleton and all.
                    if (!view.error_code) return loadDaemon(source, index);
                    // Still down, and possibly down differently: the panel is replaced
                    // only when the reason changed.
                    const stale = previous.error_code !== view.error_code;
                    state.daemonViews[source.id] = view;
                    if (stale) replaceIfIdle('daemon-detail-' + index, function () {
                        return daemonPanel(source, index);
                    });
                    return;
                }
                // A transient error does not outrank data: the last good reading is
                // kept and its age keeps counting, exactly as a dropped connection is
                // handled below.
                if (view.error_code) return;
                if (plan === 'light') view = PSL.mergeLight(kept, view);
                // Against what was on screen a moment ago, so the badge answers "what
                // changed while I was looking at it" rather than comparing two reads
                // the reader never saw next to each other.
                state.daemonMoves[source.id] = {
                    traces: PSL.gaugeMove(kept && kept.traces, view.traces),
                    analysis_queue: PSL.gaugeMove(kept && kept.analysis_queue, view.analysis_queue),
                    findings: PSL.gaugeMove(kept && kept.findings, view.findings)
                };
                state.daemonViews[source.id] = view;
                state.daemonReadAt[source.id] = Date.now();
                if (plan === 'full') state.daemonFullReadAt[source.id] = Date.now();
            })
            .catch(function () {
                // Keep the last good reading rather than blanking the panel, and let
                // its age say how stale it now is.
                state.daemonViews[source.id] = previous;
            })
            .finally(function () {
                delete state.daemonInFlight[source.id];
                const view = state.daemonViews[source.id];
                if (!view || view === 'loading' || view.error_code) return;
                if (!replaceIfIdle('daemon-top-' + index, function () {
                    return daemonTopRow(source, view, index);
                })) return;
                tickRefresh(source, index);
            });
    }

    function gaugeText(gauge) {
        if (!gauge || gauge.value == null) return t('daemon.unknown');
        if (gauge.capacity == null) return group(gauge.value);
        return group(gauge.value) + ' / ' + group(gauge.capacity);
    }

    /**
     * The settings, behind one click. They are the long part of the view and the
     * least urgent: an operator opens this row to see whether the daemon is
     * coping, and reads the settings once it is not.
     */
    function settingsDisclosure(source, view) {
        const open = state.daemonSettingsOpen[source.id] === true;
        const count = view.config ? Object.keys(view.config).length : 0;
        const cards = el('div', {class: 'settings-cards'},
            settingsColumns(settingsCards(source.id, view)));
        // The preamble sits above the columns rather than inside them, or it would
        // flow into the first one as if it were a card.
        const body = el('div', {},
            [view.config ? settingsPreamble(view) : configAbsenceNote(view), cards]);
        body.hidden = !open;

        const button = el('button', {
            type: 'button',
            class: 'settings-more',
            'aria-expanded': open ? 'true' : 'false'
        }, [el('span', {text: count > 0 ? t('daemon.settings.more', {count: count}) : t('daemon.settings.detects')})]);
        button.addEventListener('click', function () {
            const next = button.getAttribute('aria-expanded') !== 'true';
            button.setAttribute('aria-expanded', next ? 'true' : 'false');
            state.daemonSettingsOpen[source.id] = next;
            saveFolds();
            body.hidden = !next;
        });
        return el('div', {class: 'settings-block'}, [button, body]);
    }

    // Fixed, and not measured: the table around this sets min-width: 1090px and
    // scrolls rather than shrink, so the room here never falls below three.
    const SETTINGS_COLUMNS = 3;

    /**
     * Deals the cards into independent columns, round-robin so they still read
     * left to right. Not a grid: a grid row is as tall as its tallest card, so
     * opening one card pushed down every card in the rows below it, across all
     * columns. Here a card only ever moves the cards under it in its own column.
     * Not CSS columns either, which repack by height and so re-shuffled cards
     * sideways under the reader's own click.
     */
    function settingsColumns(cards) {
        // Never more columns than cards: an empty one still takes its share of the
        // width, so three of them would render a lone card at a third of the row.
        const count = Math.min(SETTINGS_COLUMNS, cards.length);
        const columns = [];
        for (let index = 0; index < count; index++)
            columns.push(el('div', {class: 'settings-col'}));
        cards.forEach(function (card, index) {
            columns[index % count].appendChild(card);
        });
        return columns;
    }

    /**
     * Why no settings are shown, as prose above the cards rather than dealt into
     * a column as if it were one: a paragraph in a third of the row reads as a
     * card that lost its rows.
     */
    function configAbsenceNote(view) {
        const reason = view.config_unavailable_reason;
        return proseInto(el('p', {class: 'daemon-lead'}),
            t(reason === 'api_disabled' ? 'daemon.config.api_disabled'
                : reason === 'unreadable' ? 'daemon.config.unreadable' : 'daemon.config.unanswered'));
    }

    function settingsCards(sourceId, view) {
        const cards = [];
        if (view.config) {
            DAEMON_GROUPS.forEach(function (spec) {
                const present = spec[1].filter(function (name) {
                    return view.config[name] !== undefined;
                });
                if (present.length > 0) {
                    cards.push(settingsCard(sourceId, spec[0], present, view.config, view.daemon_defaults));
                }
            });
        }
        if (view.detection_config) {
            cards.push(settingsCard(
                sourceId,
                'detection',
                Object.keys(view.detection_config),
                view.detection_config,
                view.detection_defaults));
        }
        if (view.scoring_config || view.energy_source) {
            const scoring = Object.assign({}, view.scoring_config || {});
            if (view.energy_source) scoring.energy_source = view.energy_source;
            cards.push(settingsCard(
                sourceId,
                'scoring',
                Object.keys(scoring),
                scoring,
                null));
        }
        return cards;
    }

    /**
     * What the marking means, and what it is worth. The defaults belong to the
     * binary this Hub embeds, which is the same approximation the daemon's own
     * monitor makes against the binary running it, so the version is named
     * rather than assumed away.
     */
    function settingsPreamble(view) {
        const note = proseInto(el('p', {class: 'settings-preamble'}),
            t('daemon.settings.preamble', {version: view.defaults_engine_version}));
        if (view.version && view.version !== view.defaults_engine_version) {
            proseInto(
                note.appendChild(el('span', {class: 'settings-preamble-skew'})),
                t('daemon.settings.preamble_skew', {version: view.version}));
        }
        return note;
    }

    /**
     * One group, folded. Folded is the useful state: nine headings fit on a
     * glance, and the count of departures on each says which one to open. The
     * gloss lives in the body rather than the heading, or the folded list would
     * be as long as the open one.
     */
    function settingsCard(sourceId, group, names, config, defaults) {
        const key = sourceId + '|' + group;
        const open = state.daemonGroupOpen[key] === true;
        const changed = names.filter(function (name) {
            return isChanged(name, config[name], defaults);
        }).length;

        const heading = el('span', {class: 'overline', text: t('daemon.group.' + group)});
        const button = el('button', {
            type: 'button',
            class: 'settings-card-head',
            'aria-expanded': open ? 'true' : 'false'
        }, [
            heading,
            el('span', {class: 'settings-card-n', text: String(names.length)}),
            changed > 0
                ? el('span', {
                    class: 'settings-card-changed',
                    text: t(changed === 1 ? 'daemon.settings.changed.one' : 'daemon.settings.changed.many', {count: changed})
                })
                : null
        ]);

        const body = el('div', {class: 'settings-card-body'}, [
            el('p', {class: 'sink-sub settings-card-sub', text: t('daemon.group.' + group + '.sub')})
        ]);
        body.hidden = !open;
        const rows = el('dl', {class: 'settings-rows'});
        names.forEach(function (name) {
            rows.appendChild(settingRow(name, config[name], defaults));
        });
        body.appendChild(rows);

        button.addEventListener('click', function () {
            const next = button.getAttribute('aria-expanded') !== 'true';
            button.setAttribute('aria-expanded', next ? 'true' : 'false');
            state.daemonGroupOpen[key] = next;
            saveFolds();
            body.hidden = !next;
        });
        return el('section', {class: 'settings-card'}, [button, body]);
    }

    /** A value that departs from the engine's default, when one is known. */
    function isChanged(name, value, defaults) {
        if (!defaults) return false;
        const fallback = defaults[name];
        return fallback !== undefined && JSON.stringify(value) !== JSON.stringify(fallback);
    }

    function settingRow(name, value, defaults) {
        const fallback = defaults ? defaults[name] : undefined;
        const changed = isChanged(name, value, defaults);
        const row = el('div', {class: 'setting'}, [
            el('dt', {class: 'setting-k', text: name}),
            settingValue(name, value, changed)
        ]);
        if (changed) {
            row.appendChild(el('dd', {
                class: 'setting-default',
                text: t('daemon.settings.default', {value: daemonValue(name, fallback)})
            }));
        }
        const copy = settingCopy(name);
        if (copy) row.appendChild(el('dd', {class: 'setting-note', text: copy}));
        return row;
    }

    function settingValue(name, value, changed) {
        if (name === 'environment') {
            return el('dd', {class: 'setting-v', 'data-changed': changed ? 'true' : null}, [
                el('span', {class: 'chip chip-declared', text: String(value)})
            ]);
        }
        return el('dd', {
            class: 'setting-v',
            'data-changed': changed ? 'true' : null,
            text: daemonValue(name, value)
        });
    }

    function daemonValue(name, value) {
        if (value === null) return t('daemon.value.not_set');
        if (name === 'sampling_rate' || name === 'correlation_min_confidence') return share(value);
        // Zero is not a percentage here, it switches the guard off entirely.
        if (name === 'memory_high_water_pct') return value === 0 ? t('daemon.value.off') : PSL.decimal(value) + ' %';
        if (name === 'max_payload_size') return PSL.bytes(value);
        if (Array.isArray(value)) return value.length === 0 ? t('daemon.value.none') : value.join(', ');
        if (name === 'tls_configured' || name === 'archive_configured') {
            return value ? t('daemon.value.configured') : t('daemon.value.unconfigured');
        }
        if (name === 'ack_api_key_set') return value ? t('daemon.value.set') : t('daemon.value.unset');
        // The name says _ms, so the millisecond figure stays primary: it is the one
        // that goes back into the file. PSL.dur rounds to the second, so the
        // readable form only appears once there is a second to read.
        if (/_ms$/.test(name) && typeof value === 'number') {
            return group(value) + ' ms' + (value >= 1000 ? ' (' + PSL.dur(value) + ')' : '');
        }
        if (typeof value === 'boolean') return value ? t('daemon.value.yes') : t('daemon.value.no');
        if (typeof value === 'number') return group(value);
        return String(value);
    }

    /**
     * A 0-to-1 share as a percentage, never rounded down to a zero it is not. A
     * sampling rate of 0.0005 is one trace in two thousand, which is very
     * different from none at all.
     */
    function share(value) {
        if (typeof value !== 'number') return String(value);
        const pct = value * 100;
        if (value > 0 && pct < 0.1) return PSL.decimal(value);
        return PSL.decimal(Math.round(pct * 10) / 10) + ' %';
    }


    // ---------------------------------------------------- screen: new analysis

    const QUICK_RANGES = [
        // Past 7d a scan rarely finishes inside the timeout: longer stays one custom range away.
        '15m', '30m', '1h', '3h', '6h', '12h', '24h', '2d', '7d'
    ];

    function selectedSource() {
        return runSources().find(function (source) {
            return source.id === state.form.sourceId;
        }) || null;
    }

    /** What the form offers: with access on, only where the caller holds analyses.run. */
    function runSources() {
        return (state.sources || []).filter(function (source) {
            return PSL.permits(source, 'analyses.run');
        });
    }

    /** Changing source clears both acknowledgements and closes the picker: they
     were answers about a different source. */
    function selectSource(id) {
        state.form.sourceId = id;
        saveSource(id);
        state.form.ackUnreachable = false;
        state.form.ackHeavy = false;
        state.form.pickerOpen = false;
        state.form.detection = {};
        render();
    }

    function setMode(mode) {
        state.form.mode = mode;
        // Switching clears the other field, and a trace ID takes no window at all,
        // so the picker cannot stay open behind a hidden control.
        if (mode === 'trace') {
            state.form.service = '';
            state.form.pickerOpen = false;
        } else {
            state.form.traceId = '';
        }
        render();
    }

    /**
     * Updated in place, never re-rendered. A full render replaces the range and
     * the number field mid-interaction, which drops the browser's pointer
     * capture (the handle stops following the mouse) and the text caret.
     */
    function setMaxTraces(value) {
        state.form.maxTraces = value;
        // Dropping back below the ceiling withdraws the question that was asked
        // about it.
        if (!PSL.weightBand(value, tracesCap()).needsAck) state.form.ackHeavy = false;
        refreshTraces();
        updateSubmit();
    }

    /** Everything on screen that reads maxTraces, refreshed without a re-render. */
    function refreshTraces() {
        const cap = tracesCap();
        const band = PSL.weightBand(state.form.maxTraces, cap);
        const over = band.key === 'over' || band.key === 'invalid';
        const value = String(state.form.maxTraces);

        const number = document.getElementById('traces-number');
        const slider = document.getElementById('traces-slider');
        // Assigned only when it differs, so the element the operator is dragging or
        // typing into is left alone.
        if (number && number.value !== value) number.value = value;
        if (number) {
            number.toggleAttribute('data-over', over);
            number.setAttribute('data-band', band.key);
        }
        if (slider) {
            const clamped = String(Math.min(Math.max(state.form.maxTraces, 1), cap));
            if (slider.value !== clamped) slider.value = clamped;
        }

        const chip = document.getElementById('traces-band');
        if (chip) {
            chip.textContent = band.label;
            chip.setAttribute('style', bandStyle(band));
        }

        const note = document.getElementById('traces-cap');
        if (note) {
            note.textContent = capNote(band, cap);
            note.setAttribute('data-over', over ? 'true' : 'false');
        }

        const body = document.getElementById('traces-body');
        if (body) {
            body.textContent = band.body;
            body.setAttribute('style', 'color:' + band.fg);
        }

        const slot = document.getElementById('traces-ack');
        if (!slot) return;
        if (band.needsAck) slot.replaceChildren(heavyAck());
        else slot.replaceChildren();
    }

    function renderNewScreen() {
        const section = el('section', {}, [
            ruledOverline(t('new.overline')),
            el('h1', {class: 'page-title', text: t('new.title')})
        ]);

        if (state.loading) {
            section.appendChild(el('div', {class: 'new-grid'}, [
                el('div', {class: 'card skeleton', style: 'height:280px'}),
                el('div', {class: 'card skeleton', style: 'height:280px'})
            ]));
            return section;
        }
        if (state.sourcesError) {
            section.appendChild(hubUnreachableBanner());
            return section;
        }
        if (!state.sources || state.sources.length === 0) {
            section.appendChild(el('div', {class: 'empty-state', text: t('new.no_source')}));
            return section;
        }
        if (runSources().length === 0) {
            section.appendChild(el('div', {class: 'empty-state', text: t('new.no_grant')}));
            return section;
        }

        const source = selectedSource();
        const skew = source && PSL.skew(source.producer_version);
        const right = el('div', {class: 'new-column'},
            [handoffBanner(source), traceHandoffBanner(source), parametersPanel(), costBand()]);
        const advanced = source && source.kind !== 'daemon' ? advancedPanel() : null;
        if (advanced) right.appendChild(advanced);
        if (skew) right.appendChild(skewNotice(source, skew));
        if (source && !source.reachable) right.appendChild(unreachableNotice(source));
        right.appendChild(submitRow());
        // Last, deliberately. The button is the action, and the other way of doing
        // the same thing is read after the decision rather than against it.
        right.appendChild(terminalSlot());

        section.appendChild(el('div', {class: 'new-grid'}, [sourcePanel(), right]));
        return section;
    }

    /**
     * Rebuilt in place on every keystroke rather than through render(), which
     * would take the focus out of the field being typed into.
     */
    function terminalSlot() {
        // No build queued here: submitRow's updateSubmit runs after the same
        // render and builds the panels, so a second build would be thrown away.
        state.terminalSig = null;
        return el('div', {id: 'terminal-panels', class: 'terminal-stack'});
    }

    /**
     * Rebuilt only when what it would say changed. This runs on every keystroke
     * through updateSubmit, and the command plus the overrides file are a
     * complete signature of the panels: every conditional note keys off one of
     * them or off the source, which the signature carries too.
     */
    function refreshTerminal() {
        const slot = document.getElementById('terminal-panels');
        if (!slot) return;
        const source = selectedSource();
        const sig = source
            ? source.id + '|' + (PSL.analysisCommand(source, buildRequest(source)) || '') + '|'
            + PSL.detectionToml(state.form.detection)
            : '';
        if (sig === state.terminalSig) return;
        state.terminalSig = sig;
        slot.replaceChildren.apply(slot, terminalPanels(source));
    }

    /**
     * The command, and the file it needs when thresholds were changed. Empty for
     * a daemon, which is read over HTTP and has no command line at all.
     */
    function terminalPanels(source) {
        if (!source) return [];
        const request = buildRequest(source);
        // Only to know whether there is a command at all: the block spells its own
        // line per shell, and this one is never the one shown.
        if (!PSL.analysisCommand(source, request, state.shell)) return [];

        const changed = Object.keys(state.form.detection).length;
        const trace = state.form.mode === 'trace';
        const panels = [terminalBlock({
            head: t('new.terminal.head'),
            sub: t('new.terminal.sub'),
            help: t('new.terminal.help'),
            spell: function (shellId) {
                return PSL.analysisCommand(source, request, shellId);
            },
            copyLabel: t('new.terminal.copy'),
            // Folded by default: the button above it is the way this Hub is meant to
            // be used, and this is the alternative for whoever wants it.
            fold: {
                open: state.panelOpen.terminal === true,
                onToggle: function (open) {
                    state.panelOpen.terminal = open;
                    saveFolds();
                }
            },
            notes: [
                // What this is, then what it takes to run it. Nobody goes and installs
                // a binary before knowing what the line above them does.
                t('new.terminal.same', {hours: state.status.limits.report_retention_hours}),
                t('new.terminal.prints'),
                trace ? t('new.terminal.trace') : null,
                stepsBlock([
                    engineNeed(el('span', {})),
                    !trace && !state.form.service.trim() ? t('new.terminal.fill_service') : null,
                    source.auth_header_name ? tokenStep(source) : null,
                    changed > 0 ? t(changed === 1 ? 'new.terminal.toml.one' : 'new.terminal.toml.many') : null
                ])
                // The note that used to say which shell this was quoted for: the tabs
                // above the line say it, and they let the reader take the other one.
            ]
        })];

        if (changed > 0) {
            panels.push(terminalBlock({
                head: t('new.toml.head'),
                sub: t('new.toml.sub'),
                text: PSL.detectionToml(state.form.detection),
                copyLabel: t('new.toml.copy'),
                download: 'perf-sentinel.toml',
                notes: [t('new.toml.defaults'), t('new.toml.where')]
            }));
        }

        return panels;
    }

    /** Hub:Access found no grant for this caller: the Hub answers, it reaches nothing. */
    function noGrantBanner(text) {
        return el('div', {class: 'banner', 'data-tone': 'warn'}, [el('div', {text: text})]);
    }

    function hubUnreachableBanner() {
        return el('div', {class: 'banner', 'data-tone': 'crit'}, [
            critGlyph(16),
            el('div', {text: t('new.hub_down')})
        ]);
    }

    /**
     * Where a pre-filled window came from. With the incident's row on hand the
     * window is said around the incident, the way the incidents screen says it,
     * otherwise as a length. A daemon takes no window, so under a daemon the
     * banner warns rather than letting the form look ready to run.
     */
    function handoffBanner(source) {
        const handoff = state.handoff;
        if (!handoff) return null;
        const daemon = Boolean(source && source.kind === 'daemon');
        const span = handoff.atMs == null
            ? t('new.handoff.long', {duration: PSL.dur(handoff.toMs - handoff.fromMs)})
            : t('new.handoff.around', {
                before: PSL.dur(handoff.atMs - handoff.fromMs),
                after: PSL.dur(handoff.toMs - handoff.atMs)
            });
        // Named the way the incidents screen names it, then the reach of the run
        // said plainly: an analysis takes a service and no namespace, so one
        // taken from a namespaced incident is wider than the row it came from.
        const name = handoff.namespace ? handoff.namespace + '/' + handoff.service : handoff.service;
        // `other` is the daemon's catch-all, and "the other of cart-svc" is not
        // a sentence, so it reads as the incident a missing kind reads as.
        const kind = handoff.kind === 'other' ? null : PSL.INCIDENT_KIND_LABEL[handoff.kind];
        // An episode's window names no incident: it is said as the episode it came from.
        const origin = handoff.pairKey
            ? t('new.handoff.episode', {name: name})
            : t('new.handoff.incident', {kind: kind || t('new.handoff.incident_kind'), name: name});
        return el('div', {class: 'banner', 'data-tone': daemon ? 'warn' : 'info'}, [
            daemon ? warningGlyph(16) : infoGlyph(16),
            el('div', {
                text: t('new.handoff.window', {origin: origin, span: span})
                    + (handoff.namespace ? t('new.handoff.namespace', {service: handoff.service}) : '')
                    + (daemon ? t('new.handoff.daemon') : '')
            })
        ]);
    }

    /** Where a trace in the form came from, and the pair's other trace one click away. */
    function traceHandoffBanner(source) {
        const handoff = state.traceHandoff;
        if (!handoff) return null;
        const daemon = Boolean(source && source.kind === 'daemon');
        const other = state.form.traceId === handoff.traceId ? handoff.otherTraceId : handoff.traceId;
        const swap = other ? el('button', {type: 'button', class: 'pill-button pill-sm', text: t('new.trace_handoff.swap')})
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
                    text: t('new.trace_handoff.lead')
                        + (other ? t('new.trace_handoff.other', {trace: other}) : '')
                        + (daemon ? t('new.trace_handoff.daemon') : '')
                }),
                swap
            ])
        ]);
    }

    function sourcePanel() {
        return el('div', {class: 'card source-panel'}, [
            el('div', {class: 'panel-head'}, [
                el('h2', {class: 'overline', text: t('new.source.head')}),
                el('span', {
                    class: 'panel-head-source',
                    text: PSL.sourceCountLabel(runSources().length, (state.sources || []).length)
                })
            ]),
            radioKeys(el('div', {class: 'source-list', role: 'radiogroup', 'aria-label': t('new.source.label')},
                sourceRows()), 'source'),
            el('p', {class: 'panel-note'}, [
                el('span', {class: 'panel-note-rule', 'aria-hidden': 'true'}),
                el('span', {text: t('new.source.note')})
            ])
        ]);
    }

    // Two kinds that behave differently enough to be worth separating: a daemon is
    // polled and pushes on its own, a trace backend is only read when a run asks.
    // The fleet table groups on the same rule, from the same helper.
    function sourceRows() {
        const sources = runSources();
        const kinds = PSL.splitByKind(sources);
        if (!kinds.split) return sources.map(sourceRadio);
        const radios = function (group) {
            return group.map(function (entry) {
                return sourceRadio(entry.source);
            });
        };
        return [sourceGroupLabel(t('sources.group.daemons'))].concat(
            radios(kinds.daemons), [sourceGroupLabel(t('sources.group.backends'))], radios(kinds.backends));
    }

    // aria-hidden: each row already names its own kind, so the label would only
    // repeat it, and it is not a radio the group should offer.
    function sourceGroupLabel(text) {
        return el('div', {class: 'source-group', role: 'presentation', 'aria-hidden': 'true'}, [
            el('span', {class: 'overline', text: text}),
            el('span', {class: 'source-group-rule'})
        ]);
    }

    let sourceMetaCount = 0;

    function sourceRadio(source) {
        const selected = source.id === state.form.sourceId;
        const now = Date.now();
        const line1 = el('div', {class: 'source-line'}, [
            el('span', {class: 'source-name', text: source.name}),
            el('span', {class: 'health', 'data-health': source.reachable ? 'ok' : 'crit'}, [
                el('span', {class: 'health-dot'}),
                el('span', {
                    text: source.reachable
                        ? t('health.reachable')
                        : t('health.unreachable', {duration: PSL.dur(now - source.unreachable_since_ms)})
                })
            ])
        ]);

        const metaId = 'source-meta-' + (++sourceMetaCount);
        const line2 = el('div', {class: 'source-line source-meta', id: metaId}, [
            el('span', {class: 'chip', text: PSL.KIND_LABEL[source.kind] || source.kind}),
            el('span', {class: 'chip chip-declared', text: source.environment}),
            el('span', {class: 'source-version', text: producerLabel(source)})
        ]);
        const gap = PSL.skew(source.producer_version);
        if (gap) line2.appendChild(el('span', {class: 'skew-pill', 'data-dir': gap.dir, text: gap.label}));

        const button = el('button', {
            type: 'button',
            class: 'source-row',
            role: 'radio',
            'aria-checked': selected ? 'true' : 'false',
            // The name and its health, then the chips as a description, rather than every chip run into the name.
            'aria-label': t(source.reachable ? 'new.source.aria_reachable' : 'new.source.aria_unreachable',
                {name: source.name}),
            'aria-describedby': metaId
        }, [el('span', {class: 'source-dot'}), el('span', {}, [line1, line2])]);
        button.addEventListener('click', function () {
            selectSource(source.id);
        });
        return button;
    }

    function producerLabel(source) {
        if (source.producer_version) return t('new.source.producer', {version: source.producer_version});
        return source.kind === 'daemon' ? t('new.source.producer_unknown') : t('new.source.no_producer');
    }

    function parametersPanel() {
        const source = selectedSource();
        if (!source) {
            return el('div', {class: 'card params-panel'}, [
                el('div', {class: 'empty-state', text: t('new.params.pick')})
            ]);
        }

        const head = el('div', {class: 'panel-head'}, [
            el('h2', {class: 'overline', text: source.kind === 'daemon' ? t('new.params.head') : t('new.query.head')}),
            el('span', {class: 'panel-head-source', text: source.name})
        ]);

        const panel = el('div', {class: 'card params-panel'}, [head]);
        if (source.kind === 'daemon') panel.appendChild(daemonNotice());
        else backendControls(source).forEach(function (node) {
            panel.appendChild(node);
        });
        return panel;
    }

    function daemonNotice() {
        return el('div', {class: 'notice'}, [
            svg([['circle', {cx: '12', cy: '12', r: '9'}], ['path', {d: 'M12 11v5M12 8.2v.2'}]], 16),
            el('div', {}, [
                el('p', {text: t('new.daemon.none')}),
                el('p', {class: 'notice-sub', text: t('new.daemon.sub')}),
                selectedSource() && PSL.permits(selectedSource(), 'sources.read') ? sourcesRowLink() : null
            ])
        ]);
    }

    /** The label promises an open row, so the row arrives open: the sources
     screen reads the flag and runs the first read itself. */
    function sourcesRowLink() {
        const link = el('a', {class: 'pill-button pill-sm', href: '#/sources', text: t('new.daemon.open_row')});
        link.addEventListener('click', function () {
            const chosen = selectedSource();
            if (chosen) {
                state.daemonOpen[chosen.id] = true;
                saveFolds();
            }
        });
        return link;
    }

    function backendControls(source) {
        const nodes = [modeSwitch()];
        if (state.form.mode === 'trace') {
            nodes.push(field(t('new.field.trace'), traceInput()));
            nodes.push(el('p', {class: 'field-note', text: t('new.field.trace_note')}));
            return nodes;
        }

        nodes.push(field(t('new.field.service'), serviceInput()));
        nodes.push(field(t('new.field.range'), rangeControl(source), state.form.rangeMode === 'absolute'
            ? t('new.field.range_absolute')
            : t('new.field.range_relative')));
        nodes.push(maxTracesBlock());
        return nodes;
    }

    function modeSwitch() {
        const group = el('div', {class: 'segmented', role: 'radiogroup', 'aria-label': t('new.mode.label')});
        [['service', t('new.mode.service')], ['trace', t('new.field.trace')]].forEach(function (entry) {
            const button = el('button', {
                type: 'button',
                role: 'radio',
                'aria-checked': state.form.mode === entry[0] ? 'true' : 'false',
                text: entry[1]
            });
            button.addEventListener('click', function () {
                setMode(entry[0]);
            });
            group.appendChild(button);
        });
        return field(t('new.mode.field'), radioKeys(group, 'mode'), t('new.mode.gloss'));
    }

    function serviceInput() {
        const input = el('input', {
            type: 'text',
            class: 'input',
            value: state.form.service,
            placeholder: 'order-service',
            spellcheck: 'false'
        });
        input.addEventListener('input', function () {
            state.form.service = input.value;
            updateSubmit();
        });
        return input;
    }

    function traceInput() {
        const input = el('input', {
            type: 'text',
            class: 'input',
            value: state.form.traceId,
            placeholder: '4bf92f3577b34da6a3ce929d0e0e4736',
            spellcheck: 'false'
        });
        input.addEventListener('input', function () {
            state.form.traceId = input.value;
            updateSubmit();
        });
        return input;
    }

    /**
     * What a radio group owes the keyboard: one tab stop on the checked radio,
     * arrows to move the choice. A choice re-renders the screen, so the focus is
     * put back on the radio at the same place in the new one.
     */
    function radioKeys(group, name) {
        const radios = Array.prototype.slice.call(group.querySelectorAll('[role="radio"]'));
        const checked = radios.findIndex(function (radio) {
            return radio.getAttribute('aria-checked') === 'true';
        });
        radios.forEach(function (radio, index) {
            radio.setAttribute('data-radio', name + '-' + index);
            radio.tabIndex = index === Math.max(checked, 0) ? 0 : -1;
        });
        group.addEventListener('click', function (event) {
            const radio = event.target.closest('[role="radio"]');
            const again = radio && document.querySelector('[data-radio="' + radio.getAttribute('data-radio') + '"]');
            if (again && again !== radio) again.focus();
        });
        group.addEventListener('keydown', function (event) {
            const step = {ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1}[event.key];
            const at = radios.indexOf(event.target);
            if (!step || at < 0) return;
            event.preventDefault();
            radios[(at + step + radios.length) % radios.length].click();
        });
        return group;
    }

    let fieldCount = 0;

    /**
     * A form row. A text control gets a real label, so it is named and a click
     * on the words focuses it. A radio group is named by aria-labelledby. The
     * gloss stays outside the label and describes the control instead.
     */
    function field(label, control, gloss) {
        const id = 'field-' + (++fieldCount);
        const labelable = /^(INPUT|SELECT|TEXTAREA)$/.test(control.tagName);
        if (labelable && !control.id) control.id = id;
        const name = labelable
            ? el('label', {id: id + '-label', for: control.id, text: label})
            : el('span', {id: id + '-label', text: label});
        if (!labelable && control.getAttribute('role')) control.setAttribute('aria-labelledby', id + '-label');
        const heading = el('span', {class: 'field-label'}, [name]);
        if (gloss) {
            heading.appendChild(el('span', {class: 'field-gloss', id: id + '-gloss', text: gloss}));
            if (labelable) control.setAttribute('aria-describedby', id + '-gloss');
        }
        return el('div', {class: 'field'}, [heading, control]);
    }

    function windowLabel() {
        if (state.form.rangeMode === 'absolute') {
            return PSL.dateTime(state.form.fromMs) + ' → ' + PSL.dateTime(state.form.toMs);
        }
        return t('new.range.last', {span: PSL.humanDur(state.form.lookback)});
    }

    /** The span, and the argument the run will actually carry. */
    function rangeWire() {
        // The argument names are the engine's, so they stay as they are sent.
        const wire = state.form.rangeMode === 'absolute' ? 'from_ms/to_ms' : ['lookback', state.form.lookback].join(' = ');
        return PSL.dur(windowSpanMs()) + ' · ' + wire;
    }

    function windowSpanMs() {
        return state.form.rangeMode === 'absolute'
            ? state.form.toMs - state.form.fromMs
            : PSL.parseDur(state.form.lookback);
    }

    function rangeControl(source) {
        const button = el('button', {
            type: 'button',
            class: 'range-pill',
            id: 'range-pill',
            'aria-label': t('new.range.aria', {window: windowLabel()}),
            'aria-expanded': String(state.form.pickerOpen)
        }, [
            svg([['circle', {cx: '12', cy: '12', r: '9'}], ['path', {d: 'M12 7v5l3.2 2'}]], 14),
            el('span', {class: 'range-pill-label', text: windowLabel()}),
            svg([['path', {d: 'M6 9l6 6 6-6'}]], 11)
        ]);
        const row = el('div', {class: 'range-row'}, [
            button,
            el('span', {class: 'range-wire', text: rangeWire()})
        ]);
        const wrap = el('div', {class: 'range'}, [row]);
        button.addEventListener('click', function () {
            if (state.form.pickerOpen) {
                closePicker();
                return;
            }
            // In place: re-rendering the form to show a panel would drop what the
            // rest of it holds in focus, caret and slider capture.
            state.form.pickerOpen = true;
            button.setAttribute('aria-expanded', 'true');
            row.after(rangePicker());
            const first = wrap.querySelector('.picker input');
            if (first) first.focus();
        });
        if (state.form.pickerOpen) wrap.appendChild(rangePicker());
        const notes = rangeConsequences(source);
        if (notes.length > 0) wrap.appendChild(el('div', {class: 'consequences'}, notes));
        return wrap;
    }

    /** Consequences appear under the control, not after the run. */
    function rangeConsequences(source) {
        const notes = [];
        const spanMs = windowSpanMs();
        if (spanMs > 86400000) {
            notes.push(consequence(t('new.range.wide')));
        }
        if (spanMs > 7 * 86400000) {
            notes.push(consequence(t('new.range.timeout', {seconds: state.status.limits.analysis_timeout_seconds})));
        }
        if (source.retention_hours != null && spanMs > source.retention_hours * 3600000) {
            notes.push(consequence(t('new.range.retention', {duration: PSL.dur(source.retention_hours * 3600000)}),
                'warn'));
        } else if (source.retention_hours == null && spanMs > 86400000) {
            notes.push(consequence(t('new.range.undeclared')));
        }
        return notes;
    }

    function consequence(text, tone) {
        return el('span', {class: 'consequence', 'data-tone': tone || 'muted'}, [
            el('span', {class: 'consequence-dot'}),
            el('span', {text: text})
        ]);
    }

    function rangePicker() {
        const backdrop = el('div', {class: 'picker-backdrop'});
        backdrop.addEventListener('click', closePicker);

        const from = el('input', {type: 'datetime-local', class: 'input-date', value: PSL.dtLocal(state.form.fromMs)});
        const to = el('input', {type: 'datetime-local', class: 'input-date', value: PSL.dtLocal(state.form.toMs)});
        const note = el('span', {class: 'picker-note'});
        const apply = el('button', {type: 'button', class: 'picker-apply', text: t('new.picker.apply_range')});

        function readAbsolute() {
            const start = Date.parse(from.value);
            const end = Date.parse(to.value);
            const ordered = Number.isFinite(start) && Number.isFinite(end) && start < end;
            const past = Number.isFinite(end) && end <= Date.now();
            const valid = ordered && past;
            note.textContent = !ordered
                ? t('new.picker.order')
                : !past
                    ? t('new.picker.future')
                    : t('new.picker.selected', {duration: PSL.dur(end - start)});
            note.setAttribute('data-invalid', valid ? 'false' : 'true');
            apply.disabled = !valid;
            return {start: start, end: end, valid: valid};
        }

        from.addEventListener('input', readAbsolute);
        to.addEventListener('input', readAbsolute);
        apply.addEventListener('click', function () {
            const read = readAbsolute();
            if (!read.valid) return;
            applyRange('absolute', {fromMs: read.start, toMs: read.end});
        });

        const left = el('div', {class: 'picker-pane'}, [
            dateField(t('new.picker.from'), from),
            dateField(t('new.picker.to'), to),
            el('div', {class: 'picker-apply-row'}, [apply, note]),
            el('div', {class: 'picker-rule'}),
            el('p', {class: 'overline', text: t('new.picker.custom')}),
            customRelativeRow()
        ]);

        const right = el('div', {class: 'picker-pane picker-right'}, [
            el('p', {class: 'overline picker-quick-head', text: t('new.picker.quick')}),
            el('div', {class: 'picker-quick'}, QUICK_RANGES.map(function (value) {
                const active = state.form.rangeMode === 'relative' && state.form.lookback === value;
                const button = el('button', {
                    type: 'button',
                    class: 'picker-quick-item',
                    'aria-current': active ? 'true' : null,
                    text: t('new.range.last', {span: PSL.humanDur(value)})
                });
                button.addEventListener('click', function () {
                    applyRange('relative', {lookback: value});
                });
                return button;
            }))
        ]);

        readAbsolute();
        return el('div', {class: 'picker-host'}, [backdrop, el('div', {class: 'picker'}, [left, right])]);
    }

    /** Closing changes nothing on the form, so the panel goes and nothing else is drawn again. */
    function closePicker() {
        state.form.pickerOpen = false;
        const host = document.querySelector('.picker-host');
        if (host) host.remove();
        const button = document.getElementById('range-pill');
        if (!button) return;
        button.setAttribute('aria-expanded', 'false');
        button.focus();
    }

    function applyRange(mode, values) {
        state.form.rangeMode = mode;
        Object.keys(values).forEach(function (key) {
            state.form[key] = values[key];
        });
        state.form.pickerOpen = false;
        renderFocus('#range-pill');
    }

    function dateField(label, control) {
        return el('label', {class: 'picker-field'}, [
            el('span', {class: 'picker-field-label', text: label}),
            control
        ]);
    }

    function customRelativeRow() {
        const qty = el('input', {
            type: 'number',
            class: 'input-qty',
            min: '1',
            value: String(state.form.customQty)
        });
        const units = el('div', {class: 'segmented segmented-sm', role: 'radiogroup', 'aria-label': t('new.picker.unit')});
        // The value stays the engine's own unit, only its label follows the language.
        [['m', 'unit.min'], ['h', 'unit.h'], ['d', 'unit.d']].forEach(function (entry) {
            const unit = entry[0];
            const button = el('button', {
                type: 'button',
                role: 'radio',
                'aria-checked': state.form.customUnit === unit ? 'true' : 'false',
                text: t(entry[1])
            });
            // Picking a unit selects it. Applying is a separate, deliberate click,
            // so a half-typed quantity is never submitted by choosing a unit.
            button.addEventListener('click', function () {
                state.form.customUnit = unit;
                state.form.customQty = Math.max(1, Number(qty.value) || 1);
                render();
            });
            units.appendChild(button);
        });
        radioKeys(units, 'unit');

        const apply = el('button', {type: 'button', class: 'pill-button pill-sm', text: t('new.picker.apply')});
        apply.addEventListener('click', function () {
            const quantity = Math.max(1, Number(qty.value) || 1);
            state.form.customQty = quantity;
            applyRange('relative', {lookback: quantity + state.form.customUnit});
        });

        return el('div', {class: 'picker-custom'}, [
            el('span', {class: 'picker-custom-lead', text: t('new.picker.last')}),
            qty,
            units,
            apply
        ]);
    }

    function maxTracesBlock() {
        const cap = state.status.limits.max_traces_cap;
        const band = PSL.weightBand(state.form.maxTraces, cap);
        const over = band.key === 'over' || band.key === 'invalid';

        const number = el('input', {
            type: 'number',
            id: 'traces-number',
            class: 'input input-traces',
            min: '1',
            max: String(cap),
            value: String(state.form.maxTraces)
        });
        if (over) number.setAttribute('data-over', 'true');
        number.setAttribute('data-band', band.key);
        number.addEventListener('input', function () {
            setMaxTraces(Number(number.value));
        });

        const head = el('div', {class: 'traces-head'}, [
            number,
            el('span', {id: 'traces-band', class: 'band-chip', style: bandStyle(band), text: band.label}),
            el('span', {
                id: 'traces-cap',
                class: 'traces-cap',
                'data-over': over ? 'true' : 'false',
                text: capNote(band, cap)
            })
        ]);

        const slider = el('input', {
            type: 'range',
            id: 'traces-slider',
            min: '1',
            max: String(cap),
            step: '1',
            value: String(Math.min(Math.max(state.form.maxTraces, 1), cap)),
            'aria-label': t('new.traces.label')
        });
        slider.addEventListener('input', function () {
            setMaxTraces(Number(slider.value));
        });

        // The container carries the pill radius and clips its children, so only the
        // outer ends are rounded and the two inner joins stay square.
        const segments = el('div', {class: 'band-segs', 'aria-hidden': 'true'},
            bands(cap).map(function (band) {
                const segment = el('span', {class: 'band-seg', 'data-seg': band.tone});
                segment.style.width = band.width;
                return segment;
            }));

        const block = el('div', {class: 'field'}, [
            // A div and not a label: the "?" is a button, which a label may not
            // contain, and the real binding is the `for` on the name beside it.
            el('div', {class: 'field-label'}, [
                el('label', {for: 'traces-number', text: t('new.traces.label')}),
                helpDot(t('new.traces.help'), t('new.traces.topic')),
                el('span', {class: 'field-gloss', text: t('new.traces.gloss')})
            ]),
            head,
            el('div', {class: 'band-track'}, [segments, slider]),
            bandScale(cap),
            el('p', {id: 'traces-body', class: 'band-body', style: 'color:' + band.fg, text: band.body}),
            // A slot rather than a conditional child: the acknowledgement appears and
            // disappears as the count crosses the ceiling, and refreshing it in place
            // keeps the rest of the block untouched.
            el('div', {id: 'traces-ack'}, band.needsAck ? [heavyAck()] : [])
        ]);
        block.appendChild(sinkPanel());
        // A slot rather than a conditional child, like traces-ack above: the
        // deferred runs fetch fills it in place without touching the form.
        const slot = el('div', {id: 'weight-history'});
        const history = weightHistory();
        if (history) slot.appendChild(history);
        block.appendChild(slot);
        return block;
    }

    /** The sink-panel scaffold both info blocks share: head, subtitle, rows. */
    function sinkBlock(head, sub, rows, fold) {
        const body = el('dl', {class: 'sink-rows'}, rows.flatMap(function (row) {
            return [el('dt', {text: row[0]}), el('dd', {text: row[1]})];
        }));
        if (!fold) {
            return el('div', {class: 'sink'}, [
                el('div', {class: 'sink-head'}, [
                    el('h2', {class: 'overline', text: head}),
                    el('span', {class: 'sink-sub', text: sub})
                ]),
                body
            ]);
        }

        // The same fold as the daemon row's terminal block, down to the chevron:
        // the heading and its subtitle stay put, only the rows go.
        const toggle = el('button', {
            type: 'button',
            class: 'sink-more',
            'aria-expanded': fold.open ? 'true' : 'false'
        }, [el('span', {class: 'overline', text: head})]);
        body.hidden = !fold.open;
        const root = el('div', {class: 'sink'}, [
            el('div', {class: 'sink-head'}, [toggle, el('span', {class: 'sink-sub', text: sub})]),
            body
        ]);
        // Marked rather than inferred from the hidden body: the heading's own
        // bottom margin has nothing under it when folded, and .sink-head is shared
        // with three blocks that do want it.
        root.toggleAttribute('data-folded', !fold.open);
        toggle.addEventListener('click', function () {
            const next = toggle.getAttribute('aria-expanded') !== 'true';
            toggle.setAttribute('aria-expanded', next ? 'true' : 'false');
            body.hidden = !next;
            root.toggleAttribute('data-folded', !next);
            fold.onToggle(next);
        });
        return root;
    }

    /**
     * What this source's own runs weighed, at the count they asked for. A
     * measurement rather than a model: a report's size follows how many spans
     * its traces carry, which differs per service and which the launcher cannot
     * know before the run. Absent until this source has a measured run.
     */
    function weightHistory() {
        const source = selectedSource();
        if (!source || !state.runs) return null;
        const past = state.runs.filter(function (run) {
            const result = run.result || {};
            // Trace-id runs are excluded: their weight says nothing about the
            // slider. A missing max_traces on a service run is the server-side
            // default, not an unmeasured one.
            return run.source_id === source.id &&
                run.status === 'succeeded' &&
                typeof result.report_bytes === 'number' &&
                (run.request || {}).trace_id === undefined;
        }).slice(0, 3);
        if (past.length === 0) return null;

        return sinkBlock(t('new.weighed.head'), t('new.weighed.sub'),
            past.map(function (run) {
                const traces = typeof run.request.max_traces === 'number' ? run.request.max_traces : 100;
                return [PSL.bytes(run.result.report_bytes), t('new.weighed.row', {
                    traces: traces,
                    findings: run.result.findings,
                    duration: PSL.dur(Date.now() - (run.finished_at_ms || run.created_at_ms))
                })];
            }));
    }

    function capNote(band, cap) {
        if (band.key === 'invalid') return t('new.traces.at_least');
        if (band.key === 'over') return t('new.traces.over', {cap: cap});
        return t('new.traces.cap', {cap: cap});
    }

    /**
     * The bands that exist at this cap, each with the boundary it ends at and
     * its share of the rule. A cap at or below a boundary means that band never
     * occurs: it is dropped rather than drawn at zero width under a label
     * repeating the cap.
     */
    function bands(cap) {
        const inner = [['ok', 500, 'new.band.safe'], ['warn', 1200, 'new.band.heavy']]
            .filter(function (band) {
                return band[1] < cap;
            });
        // The stripe that ends at the cap takes the tone of whichever band the cap
        // itself falls in, which is the one after the last inner boundary kept.
        // Always painting it crit would turn a Hub capped at 500 into an entirely
        // red rule over a range that is all comfortable.
        const TONES = ['ok', 'warn', 'crit'];
        const kept = inner.concat([[TONES[inner.length], cap, 'new.band.cap']]);

        let previous = 0;
        return kept.map(function (band) {
            const share = (band[1] - previous) / cap;
            previous = band[1];
            return {
                tone: band[0],
                label: group(band[1]) + ' ' + t(band[2]),
                width: (share * 100).toFixed(2) + '%'
            };
        });
    }

    /** Each label sits at the right edge of its own band, so it marks a boundary. */
    function bandScale(cap) {
        const current = bands(cap);
        const grid = el('div', {class: 'band-scale-grid'}, current.map(function (band) {
            return el('span', {text: band.label});
        }));
        grid.style.gridTemplateColumns = current.map(function (band) {
            return band.width;
        }).join(' ');
        return el('div', {class: 'band-scale'}, [
            el('span', {class: 'band-scale-start', text: '1'}),
            grid
        ]);
    }

    function group(value) {
        // Thousands separators on the integer part only: a decimal such as
        // 0.6667 must not come out as 0.6 667.
        const text = String(value);
        const point = text.indexOf('.');
        const whole = point < 0 ? text : text.slice(0, point);
        return whole.replace(/\B(?=(\d{3})+(?!\d))/g, '\u202f') + (point < 0 ? '' : PSL.decimal(text.slice(point)));
    }

    // render() returns early when the status is missing, so every caller runs
    // with one in hand.
    function tracesCap() {
        return state.status.limits.max_traces_cap;
    }

    function embeddedCap() {
        return state.status.limits.max_traces_embedded;
    }

    /**
     * What the sink guarantees, measured rather than predicted. The design bans
     * predicting a byte size, and its reason still holds: SQL template lengths
     * move a fixed-count report by tens of kilobytes. Both ends of the range are
     * fixed, though, so they can be stated as facts.
     */
    function sinkPanel() {
        const rows = [
            ['550 ' + t('unit.kb'), t('new.sink.floor')],
            [t('new.sink.every'), t('new.sink.every_body')],
            [t('new.sink.trees', {count: embeddedCap()}), t('new.sink.trees_body')],
            ['25', t('new.sink.offenders')],
            [t('new.sink.no_ceiling'), t('new.sink.no_ceiling_body')]
        ];
        // Folded until the reader opens it, like every other fold in the product,
        // and remembered from then on.
        return sinkBlock(t('new.sink.head'), t('new.sink.sub'), rows, {
                open: state.panelOpen.caps === true,
                onToggle: function (open) {
                    state.panelOpen.caps = open;
                    saveFolds();
                }
            });
    }

    function bandStyle(band) {
        return 'color:' + band.fg + ';background:' + band.bg;
    }

    function heavyAck() {
        const node = checkbox(
            state.form.ackHeavy,
            t('new.traces.ack'),
            function (checked) {
                state.form.ackHeavy = checked;
                updateSubmit();
            });
        node.classList.add('checkbox-pill');
        return node;
    }

    /**
     * A producer behind the engine is worth saying out loud: perf-sentinel is
     * pre-1.0, so a detector added between minors does not run on the older
     * binary at all, and its absence looks exactly like a clean service.
     */
    function skewNotice(source, skew) {
        const engine = state.status.engine_version;
        const behind = skew.dir === 'behind';
        return el('section', {class: 'notice-block', 'data-tone': behind ? 'warn' : 'info'}, [
            warningGlyph(16),
            el('div', {class: 'notice-block-text'}, [
                el('p', {
                    class: 'notice-block-title',
                    text: t('new.skew.title', {
                        name: source.name, version: source.producer_version, gap: skew.label, engine: engine
                    })
                }),
                el('p', {
                    class: 'notice-block-body',
                    text: behind ? t('new.skew.behind', {engine: engine}) : t('new.skew.ahead')
                }),
                behind ? upgradeLine(source, engine) : null
            ])
        ]);
    }

    /**
     * Where the newer version comes from, which is not the same place for every
     * source. A daemon is upgraded through its chart, so pointing its operator at
     * a binary would be pointing at the wrong artefact.
     */
    function upgradeLine(source, engine) {
        const line = el('p', {class: 'notice-block-body'}, [
            el('span', {text: t('new.upgrade.get', {engine: engine})}),
            el('a', {
                class: 'notice-block-link',
                href: PSL.releaseUrl(engine),
                target: '_blank',
                rel: 'noopener noreferrer',
                text: t('new.upgrade.release')
            })
        ]);
        if (source.kind !== 'daemon') return line;
        line.appendChild(el('span', {text: t('new.upgrade.or_chart')}));
        line.appendChild(el('a', {
            class: 'notice-block-link',
            href: PSL.CHART_PAGE,
            target: '_blank',
            rel: 'noopener noreferrer',
            text: t('new.upgrade.charts')
        }));
        line.appendChild(el('span', {text: t('new.upgrade.chart_is')}));
        // A coordinate for helm, not a link: the scheme is not one a browser opens.
        line.appendChild(el('code', {class: 'code-inline', text: PSL.CHART_COORDINATE}));
        line.appendChild(el('span', {text: t('new.upgrade.end')}));
        return line;
    }

    function unreachableNotice(source) {
        const text = el('div', {class: 'notice-block-text'}, [
            el('p', {
                class: 'notice-block-title',
                text: t('new.unreachable.title', {
                    name: source.name, duration: PSL.dur(Date.now() - source.unreachable_since_ms)
                })
            })
        ]);
        if (source.last_success_ms) {
            text.appendChild(el('p', {
                class: 'notice-block-body',
                text: t('new.unreachable.last_success', {duration: PSL.dur(Date.now() - source.last_success_ms)})
            }));
        }
        if (source.last_error_code) {
            text.appendChild(el('p', {class: 'notice-block-body'}, [
                el('span', {text: t('new.unreachable.returned')}),
                el('span', {class: 'code-inline', text: source.last_error_code}),
                proseInto(el('span'), t('daemon.error.meaning', {
                    meaning: PSL.READ_ERRORS[source.last_error_code] || PSL.ERRORS[source.last_error_code]
                        || t('daemon.error.unreached')
                }))
            ]));
        }
        text.appendChild(el('p', {class: 'notice-block-body', text: t('new.unreachable.cost')}));
        text.appendChild(checkbox(
            state.form.ackUnreachable,
            t('new.unreachable.anyway'),
            function (checked) {
                state.form.ackUnreachable = checked;
                updateSubmit();
            }));

        return el('section', {class: 'notice-block', 'data-tone': 'warn'}, [warningGlyph(17), text]);
    }

    /** The circle-exclamation every crit banner carries, drawn once. */
    function critGlyph(size) {
        return svg([
            ['circle', {cx: '12', cy: '12', r: '9'}],
            ['path', {d: 'M12 7.5v5M12 15.8v.2'}]
        ], size);
    }

    function warningGlyph(size) {
        return svg([
            ['path', {d: 'M10.3 3.9 1.9 18a2 2 0 0 0 1.7 3h16.8a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z'}],
            ['path', {d: 'M12 9v4M12 17h.01'}]
        ], size);
    }

    function checkbox(checked, label, onChange) {
        const input = el('input', {type: 'checkbox'});
        input.checked = checked;
        input.addEventListener('change', function () {
            onChange(input.checked);
        });
        return el('label', {class: 'checkbox'}, [input, el('span', {text: label})]);
    }

    /** Reported by the service, not assumed: the button is a promise of cost. */
    function costBand() {
        const limits = state.status.limits;
        const queue = state.status.queue_depth;
        const cells = [
            [String(limits.max_traces_cap), t('new.cost.traces'), t('new.cost.cap'), t('new.cost.cap_note')],
            [String(limits.analysis_timeout_seconds), t('unit.s'), t('new.cost.timeout'), t('new.cost.timeout_note')],
            [String(state.status.workers), t('new.cost.workers'),
                t(queue === 0 ? 'new.cost.queued.none' : queue === 1 ? 'new.cost.queued.one' : 'new.cost.queued.many',
                    {count: queue}),
                t('new.cost.workers_note')],
            [String(limits.report_retention_hours), t('unit.h'), t('new.cost.retention'), t('new.cost.retention_note')]
        ];
        return el('section', {class: 'card cost'}, [
            el('div', {class: 'cost-head'}, [
                el('h2', {class: 'overline', text: t('new.cost.head')}),
                el('span', {class: 'cost-sub', text: t('new.cost.sub')})
            ]),
            el('div', {class: 'cost-grid'}, cells.map(function (cell) {
                return el('div', {class: 'cost-cell'}, [
                    el('p', {class: 'cost-figure'}, [
                        el('span', {text: cell[0]}),
                        el('span', {class: 'cost-unit', text: cell[1]})
                    ]),
                    el('p', {class: 'cost-label', text: cell[2]}),
                    el('p', {class: 'cost-note', text: cell[3]})
                ]);
            }))
        ]);
    }

    /**
     * What blocks the run, or null when nothing does. Mirrors the server's own
     * rules so the operator is told before spending a round trip.
     */
    function submitBlocker() {
        const source = selectedSource();
        if (!source) return t('new.block.source');
        if (!state.status.engine_version) return t('new.block.engine');
        if (!source.reachable && !state.form.ackUnreachable) return t('new.block.unreachable');
        if (source.kind === 'daemon') return null;
        if (state.form.mode === 'trace') {
            return state.form.traceId.trim() ? null : t('new.block.trace');
        }
        if (!state.form.service.trim()) return t('new.block.service');
        const band = PSL.weightBand(state.form.maxTraces, tracesCap());
        if (band.key === 'over') return t('new.block.over');
        if (band.key === 'invalid') return t('new.block.invalid');
        return band.needsAck && !state.form.ackHeavy ? t('new.block.heavy') : null;
    }

    /** Restates the request in a sentence, so the button is not a leap of faith. */
    function submitSentence() {
        const source = selectedSource();
        if (!source) return '';
        if (source.kind === 'daemon') {
            return t('new.submit.daemon', {name: source.name, queue: queuePhrase()});
        }
        if (state.form.mode === 'trace') {
            return t('new.submit.trace', {name: source.name, queue: queuePhrase()});
        }
        return t('new.submit.service', {
            count: state.form.maxTraces,
            service: state.form.service.trim() || t('new.submit.a_service'),
            window: state.form.rangeMode === 'absolute'
                ? t('new.submit.selected_window')
                : t('new.submit.last', {span: PSL.humanDur(state.form.lookback)}),
            name: source.name,
            queue: queuePhrase()
        });
    }

    function queuePhrase() {
        const queue = state.status.queue_depth;
        if (queue === 0) return t('new.queue.none');
        return queue === 1 ? t('new.queue.one') : t('new.queue.many', {count: queue});
    }

    function submitRow() {
        const button = el('button', {type: 'button', class: 'submit', id: 'submit'}, [
            playGlyph(),
            el('span', {text: t('new.submit.button')})
        ]);
        button.addEventListener('click', submit);
        const row = el('div', {class: 'submit-row'}, [
            button,
            el('p', {class: 'submit-sentence', id: 'submit-sentence'})
        ]);
        queueMicrotask(updateSubmit);
        return row;
    }

    function playGlyph() {
        const node = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
        node.setAttribute('viewBox', '0 0 24 24');
        node.setAttribute('width', '15');
        node.setAttribute('height', '15');
        node.setAttribute('fill', 'currentColor');
        node.setAttribute('aria-hidden', 'true');
        const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
        path.setAttribute('d', 'M7 4.5v15l13-7.5z');
        node.appendChild(path);
        return node;
    }

    function updateSubmit() {
        const button = document.getElementById('submit');
        const sentence = document.getElementById('submit-sentence');
        if (!button || !sentence) return;
        const blocker = submitBlocker();
        button.disabled = blocker !== null;
        button.title = blocker || '';
        sentence.textContent = blocker || submitSentence();
        sentence.setAttribute('data-blocked', blocker ? 'true' : 'false');
        refreshTerminal();
    }

    function buildRequest(source) {
        if (source.kind === 'daemon') return {};
        if (state.form.mode === 'trace') {
            const trace = {trace_id: state.form.traceId.trim()};
            if (Object.keys(state.form.detection).length > 0) trace.detection = state.form.detection;
            return trace;
        }
        const request = {service: state.form.service.trim(), max_traces: state.form.maxTraces};
        if (Object.keys(state.form.detection).length > 0) request.detection = state.form.detection;
        if (state.form.rangeMode === 'absolute') {
            request.from_ms = state.form.fromMs;
            request.to_ms = state.form.toMs;
        } else {
            request.lookback = state.form.lookback;
        }
        return request;
    }

    function submit() {
        const source = selectedSource();
        if (!source || submitBlocker()) return;
        const button = document.getElementById('submit');
        button.disabled = true;

        fetch('/api/analyses', {
            method: 'POST',
            headers: {'content-type': 'application/json'},
            body: JSON.stringify({source_id: source.id, request: buildRequest(source)})
        }).then(signInOn401).then(function (response) {
            return response.json().then(function (payload) {
                return {ok: response.ok, payload: payload};
            });
        }).then(function (result) {
            if (!result.ok) throw new Error(PSL.problemText(result.payload, undefined, t('new.submit.refused')));
            location.hash = '#/run/' + result.payload.id;
        }).catch(function (error) {
            const sentence = document.getElementById('submit-sentence');
            if (sentence) {
                sentence.textContent = String(error.message || error);
                sentence.setAttribute('data-blocked', 'true');
            }
            updateSubmit();
        });
    }


    // ------------------------------------------------------- screen: one run

    function renderRunScreen(id) {
        const run = state.run;
        const section = el('section', {}, [backLink()]);
        if (state.runError === id) {
            section.appendChild(unknownRun());
            return section;
        }
        if (state.runReadFailures > 0) section.appendChild(runUnreachableBanner());
        if (!run || run.id !== id) {
            section.appendChild(el('div', {class: 'card skeleton', style: 'height:220px;margin-top:16px'}));
            return section;
        }

        const key = PSL.statusKey(run);
        const view = runView(run, key);
        section.appendChild(el('div', {class: 'run-head'}, [
            el('span', {class: 'status-pill', 'data-status': key, text: t('status.' + key)}),
            el('span', {class: 'run-id', text: run.id})
        ]));
        section.appendChild(el('h1', {class: 'page-title', text: view.headline}));
        const sub = typeof view.sub === 'function' ? view.sub : null;
        section.appendChild(live(el('p', {class: 'page-sub', text: sub ? sub() : view.sub}), sub));

        const left = el('div', {class: 'run-left'}, [eventLog(run, key)]);
        const outcome = outcomePanel(run, key, view);
        if (outcome) left.appendChild(outcome);
        section.appendChild(el('div', {class: 'run-grid'}, [left, factsRail(run, key)]));
        return section;
    }

    /** A run ID the Hub does not hold, from the run screen or a report link. */
    function unknownRun() {
        return el('div', {class: 'gone'}, [
            el('h1', {class: 'page-title', text: t('run.unknown.title')}),
            el('p', {class: 'page-sub', text: t('run.unknown.sub')}),
            el('div', {class: 'outcome-actions'}, [
                el('a', {class: 'action action-filled', href: '#/new', text: t('new.title')}),
                el('a', {class: 'action action-secondary', href: '#/recent', text: t('run.all')})
            ])
        ]);
    }

    /** Back to the form, filled with what this run asked, to send as it is or change first. */
    function rerun(run) {
        Object.assign(state.form, PSL.formFromRequest(run.request),
            {sourceId: run.source_id, ackUnreachable: false, ackHeavy: false});
        state.handoff = null;
        state.traceHandoff = null;
        location.hash = '#/new';
    }

    function rerunAction(label, filled) {
        return {
            label: label, filled: filled, action: function () {
                rerun(state.run);
            }
        };
    }

    function backLink() {
        return el('a', {class: 'back-pill', href: '#/recent'}, [
            svg([['path', {d: 'M15 18l-6-6 6-6'}]], 13),
            el('span', {text: t('run.all')})
        ]);
    }

    /** Headline and sub-line per state, in the source's own terms. */
    function runView(run, key) {
        if (key === 'queued') {
            return {
                headline: t('run.queued.headline'),
                sub: t('run.queued.sub', {name: run.source_name})
            };
        }
        if (key === 'running') {
            return {
                headline: t('run.running.headline', {name: run.source_name}),
                sub: t('run.running.sub')
            };
        }
        if (key === 'empty') {
            return {
                headline: t('run.empty.headline'),
                sub: t('run.empty.sub')
            };
        }
        if (key === 'succeeded') {
            const result = run.result || {};
            const caveats = (result.warnings || []).length;
            return {
                headline: caveats > 0
                    ? t(caveats === 1 ? 'run.succeeded.headline_caveat.one' : 'run.succeeded.headline_caveat.many',
                        {findings: result.findings, caveats: caveats})
                    : t('run.succeeded.headline', {findings: result.findings}),
                sub: function () {
                    return t('run.succeeded.sub', {duration: PSL.durPrecise(run.expires_at_ms - Date.now())});
                }
            };
        }
        if (key === 'interrupted') {
            return {
                headline: t('run.interrupted.headline'),
                sub: t('run.interrupted.sub', {
                    duration: PSL.dur((run.finished_at_ms || 0) - (run.started_at_ms || run.created_at_ms))
                })
            };
        }
        if (key === 'expired') {
            return {
                headline: t('run.expired.headline'),
                sub: function () {
                    return t('run.expired.sub', {
                        hours: state.status.limits.report_retention_hours,
                        duration: PSL.durPrecise(Date.now() - run.expires_at_ms)
                    });
                }
            };
        }
        const code = run.error_code || 'internal';
        return {
            headline: PSL.hasText('error.title.' + code) && PSL.lang() !== 'en'
                ? t('error.title.' + code)
                : t('run.failed.headline', {code: String(code).replace(/_/g, ' ')}),
            sub: t('run.failed.sub')
        };
    }

    /**
     * A receipt, not a feed. Every line is a timestamp the Hub wrote: the design
     * calls for a `dequeued` line too, but this service records one instant for
     * dequeue and start, and inventing a second would be interpolation.
     */
    function eventLog(run, key) {
        const rows = [logRow(run.created_at_ms, t('run.log.accepted'), t('run.log.accepted_detail'), 'muted')];
        if (run.started_at_ms) {
            rows.push(logRow(run.started_at_ms, t('run.log.started'),
                run.kind === 'daemon'
                    ? t('run.log.started_daemon')
                    : t('run.log.started_backend', {kind: PSL.KIND_LABEL[run.kind]}),
                'brand'));
        }
        if (key === 'running') rows.push(logRow(null, t('run.log.running'), t('run.log.running_detail'), 'brand'));
        if (key === 'queued') rows.push(logRow(null, t('run.log.waiting'), t('run.log.waiting_detail'), 'muted'));
        if (key === 'succeeded' || key === 'empty') {
            rows.push(logRow(run.finished_at_ms, t('run.log.succeeded'),
                t('run.log.written_retained', {hours: state.status.limits.report_retention_hours}), 'ok'));
        }
        if (key === 'failed') rows.push(logRow(run.finished_at_ms, t('run.log.failed'), run.error_code, 'crit'));
        if (key === 'interrupted') {
            rows.push(logRow(run.finished_at_ms, t('run.log.interrupted'), t('run.log.interrupted_detail'), 'info'));
        }
        if (key === 'expired') {
            rows.push(logRow(run.finished_at_ms, t('run.log.succeeded'), t('run.log.written'), 'muted'));
            rows.push(logRow(run.expires_at_ms, t('run.log.deleted'), t('run.log.deleted_detail'), 'muted'));
        }

        return el('section', {class: 'card log-card', 'aria-label': t('run.log.label')}, [
            el('div', {class: 'log-head'}, [
                el('h2', {class: 'overline', text: t('run.log.head')}),
                el('span', {class: 'log-head-note', text: t('run.log.note')})
            ]),
            el('div', {class: 'log'}, rows),
            el('div', {class: 'log-foot'}, [el('p', {text: logClosing(run, key)})])
        ]);
    }

    function logRow(ms, name, detail, tone) {
        return el('div', {class: 'log-row'}, [
            el('span', {class: 'log-time', text: ms ? PSL.clock(ms) : '…'}),
            el('span', {class: 'log-dot', 'data-tone': tone}),
            el('span', {class: 'log-text'}, [
                el('span', {class: 'log-name', 'data-tone': tone, text: name}),
                el('span', {class: 'log-detail', text: detail})
            ])
        ]);
    }

    function logClosing(run, key) {
        if (key === 'running' || key === 'queued') return t('run.log.closing_live');
        const instant = run.finished_at_ms && (run.finished_at_ms - run.created_at_ms) < 10000;
        return instant ? t('run.log.closing_instant') : t('run.log.closing');
    }

    function factsRail(run, key) {
        const elapsedMs = key === 'queued'
            ? Date.now() - run.created_at_ms
            : (run.finished_at_ms || Date.now()) - (run.started_at_ms || run.created_at_ms);
        const figure = el('div', {class: 'elapsed', 'data-running': key === 'running' ? 'true' : 'false'});
        PSL.durParts(elapsedMs).forEach(function (part) {
            figure.appendChild(el('span', {class: 'elapsed-part'}, [
                el('span', {class: 'elapsed-n', text: part.n}),
                el('span', {class: 'elapsed-u', text: part.u})
            ]));
        });

        const elapsed = el('section', {class: 'card rail-card'}, [
            el('h2', {class: 'overline overline-p', text: t('run.elapsed')}),
            figure
        ]);
        // The only bar in the product, and only while running: it measures a known
        // ceiling, not progress toward an unknown total.
        if (key === 'running') elapsed.appendChild(ceilingRule(elapsedMs));
        elapsed.appendChild(el('p', {class: 'rail-note', text: ceilingNote(key, elapsedMs)}));

        return el('aside', {class: 'rail'}, [elapsed, requestCard(run)]);
    }

    function ceilingRule(elapsedMs) {
        const ceilingMs = state.status.limits.analysis_timeout_seconds * 1000;
        const fill = el('span', {class: 'ceiling-fill'});
        fill.style.width = Math.min(100, (elapsedMs / ceilingMs) * 100).toFixed(1) + '%';
        if (elapsedMs > ceilingMs * 0.8) fill.setAttribute('data-near', 'true');
        return el('div', {class: 'ceiling', 'aria-hidden': 'true'}, [fill]);
    }

    function ceilingNote(key, elapsedMs) {
        const seconds = state.status.limits.analysis_timeout_seconds;
        if (key === 'queued') return t('run.ceiling.queued', {seconds: seconds});
        if (key !== 'running') return t('run.ceiling.total');
        const left = seconds * 1000 - elapsedMs;
        return left <= 0
            ? t('run.ceiling.past', {seconds: seconds})
            : t('run.ceiling.left', {seconds: seconds, duration: PSL.dur(left)});
    }

    function requestCard(run) {
        const request = run.request || {};
        const facts = [
            [t('run.fact.source'), run.source_name, 'ui'],
            [t('run.fact.type'), PSL.KIND_LABEL[run.kind] || run.kind, 'mono']
        ];
        // The engine's own argument names, kept as they are sent.
        ['service', 'trace_id', 'lookback', 'max_traces'].forEach(function (name) {
            if (request[name] != null) facts.push([name, String(request[name]), 'mono']);
        });
        if (request.from_ms) {
            facts.push([t('run.fact.window'), PSL.dateTime(request.from_ms) + ' → ' + PSL.dateTime(request.to_ms), 'mono']);
        }
        Object.keys(request.detection || {}).forEach(function (name) {
            facts.push([name, String(request.detection[name]), 'warn']);
        });
        if (!request.service && !request.trace_id) facts.push([t('run.fact.parameters'), t('run.fact.none'), 'muted']);
        facts.push([t('run.fact.requested_by'), run.requested_by, 'mono']);
        facts.push([t('run.fact.detected_by'), run.producer_version
            ? t('run.fact.' + PSL.detector(run.kind)) + ' ' + run.producer_version
            : t('run.fact.unknown'), PSL.skew(run.producer_version) ? 'warn' : 'mono']);
        facts.push([t('run.fact.expires'), expiryText(run),
            run.expires_at_ms && run.expires_at_ms < Date.now() ? 'crit' : 'mono',
            function () {
                return expiryText(run);
            }]);

        return el('section', {class: 'request'}, [
            el('h2', {class: 'overline overline-p', text: t('run.request')}),
            el('div', {class: 'request-grid'}, facts.map(function (fact) {
                return el('div', {class: 'fact-card'}, [
                    el('span', {class: 'fact-card-k', text: fact[0]}),
                    live(el('span', {
                        class: 'fact-card-v',
                        'data-tone': fact[2],
                        text: fact[1],
                        title: fact[1]
                    }), fact[3])
                ]);
            }))
        ]);
    }

    function expiryText(run) {
        if (!run.expires_at_ms) return t('run.expiry.pending');
        const delta = run.expires_at_ms - Date.now();
        return delta > 0
            ? t('run.expiry.in', {duration: PSL.durPrecise(delta)})
            : t('sources.ago', {duration: PSL.durPrecise(-delta)});
    }

    function outcomePanel(run, key, _) {
        if (key === 'running' || key === 'queued') return null;
        const spec = outcomeSpec(run, key);
        const panel = el('section', {class: 'outcome', 'data-tone': spec.tone}, [
            el('h2', {class: 'overline overline-p', text: '// ' + spec.title}),
            el('p', {class: 'outcome-body', text: spec.body})
        ]);
        if (spec.counts) panel.appendChild(countStrip(spec.counts, {toned: true, filled: 1}));
        const trimmed = trimNotice(run);
        if (trimmed) panel.appendChild(trimmed);
        (spec.warnings || []).forEach(function (warning) {
            // A caveat the engine wrote, which stays in the engine's English.
            panel.appendChild(el('div', {class: 'outcome-warning'}, [
                el('span', {class: 'outcome-warning-kind', text: warning.kind}),
                engineText(el('span', {class: 'outcome-warning-message', text: warning.message}))
            ]));
        });
        panel.appendChild(actionRow(run, spec));
        return panel;
    }

    function outcomeSpec(run, key) {
        const result = run.result || {};
        if (key === 'succeeded') {
            return {
                // The run succeeding and the gate passing are two different verdicts.
                // A green panel announcing a failed gate contradicts its own sentence.
                tone: result.quality_gate_passed ? 'ok' : 'crit', title: t('run.outcome.result'),
                body: result.quality_gate_passed ? t('run.outcome.gate_passed') : t('run.outcome.gate_failed'),
                counts: [
                    [result.quality_gate_passed ? 'PASS' : 'FAIL', t('run.count.gate'),
                        result.quality_gate_passed ? 'ok' : 'crit'],
                    [String(result.findings), result.kept_findings == null ? t('run.count.findings') : t('run.count.found'),
                        result.critical > 0 ? 'crit' : result.warning > 0 ? 'warn'
                            : result.info > 0 ? 'info' : 'ok'],
                    [String(result.critical), t('run.count.critical'), 'crit'],
                    [String(result.warning), t('run.count.warning'), 'warn'],
                    [String(result.info), t('run.count.info'), 'info'],
                    [String(result.traces_analyzed), t('run.count.traces'), 'text']
                ],
                warnings: result.warnings,
                primary: {label: t('run.outcome.open'), href: '#/report/' + run.id, filled: true},
                note: function () {
                    return t('run.outcome.open_note', {duration: PSL.durPrecise(run.expires_at_ms - Date.now())});
                }
            };
        }
        if (key === 'empty') {
            return {
                tone: 'warn', title: t('run.outcome.empty'),
                body: t('run.outcome.empty_body', {name: run.source_name}),
                counts: [
                    [result.quality_gate_passed ? 'PASS' : 'FAIL', t('run.count.gate'), 'muted'],
                    [String(result.findings), t('run.count.findings'), 'warn'],
                    [String(result.traces_analyzed), t('run.count.traces'), 'warn']
                ],
                warnings: result.warnings,
                primary: rerunAction(t('run.outcome.wait_rerun'), false),
                secondary: {label: t('run.outcome.open_blank'), href: '#/report/' + run.id},
                note: t('run.outcome.empty_note')
            };
        }
        if (key === 'failed') {
            return {
                tone: 'crit', title: run.error_code || 'internal',
                body: t('run.outcome.failed_body', {
                    name: run.source_name,
                    meaning: PSL.ERRORS[run.error_code] || t('run.outcome.unnamed')
                }),
                primary: rerunAction(t('run.outcome.rerun'), false),
                secondary: {label: t('run.outcome.check_source'), href: '#/sources'},
                note: t('run.outcome.failed_note')
            };
        }
        if (key === 'interrupted') {
            return {
                tone: 'info', title: t('run.outcome.resume'),
                body: t('run.outcome.resume_body', {name: run.source_name}),
                primary: {
                    label: t('run.outcome.resume_button'),
                    action: function (button) {
                        resubmit(run, button);
                    },
                    filled: true
                },
                note: PSL.argsLine(run)
            };
        }
        return {
            tone: 'muted', title: t('run.outcome.expired'),
            body: t('run.outcome.expired_body'),
            primary: rerunAction(t('run.outcome.rerun'), false),
            note: PSL.argsLine(run)
        };
    }

    /**
     * The sink drops findings to fit its budget, and the count on the card is
     * what the engine found, not what the report holds. Said above the link,
     * because it changes how the numbers should be read.
     */
    function trimNotice(run) {
        const result = run.result || {};
        if (result.kept_findings == null || result.kept_findings >= result.findings) return null;
        return el('div', {class: 'outcome-warning'}, [
            el('span', {class: 'outcome-warning-kind', text: t('run.trimmed')}),
            el('span', {
                class: 'outcome-warning-message',
                text: t('run.trimmed_body', {found: result.findings, kept: result.kept_findings})
            })
        ]);
    }

    /**
     * A cell is [figure, label, tone, move]. `move` stays a number throughout, so
     * the fill is named by index in the options rather than by a fifth slot that
     * would sit where a facts tuple keeps something else entirely.
     *
     * `options.toned` carries the tone onto the cell, the way the rendered
     * dashboard draws the same figures: `options.filled` names the one cell shown
     * as a solid block of its tone, the rest take their own pastel. The daemon
     * gauges pass no options and keep a neutral strip, because their tone means
     * "near a cap", which is not a severity.
     */
    function countStrip(counts, options) {
        const opts = options || {};
        return el('div', {class: 'counts'}, counts.map(function (cell, index) {
            const tone = opts.toned && cell[2] && cell[2] !== 'text' ? cell[2] : null;
            const filled = index === opts.filled;
            const figure = el('span', {class: 'count-n', 'data-tone': cell[2]},
                [document.createTextNode(cell[0])]);
            if (typeof cell[3] === 'number') figure.appendChild(moveBadge(cell[3]));
            return el('div', {
                class: 'count',
                'data-grad': tone && !filled ? tone : null,
                'data-kpi': tone && filled ? tone : null
            }, [figure, el('span', {class: 'count-l', text: cell[1]})]);
        }));
    }

    /**
     * How far a gauge moved since the last read. Up is the bad direction here:
     * every one of these counts toward a cap, so a rise is ground lost and a
     * fall is ground won, which is the opposite of the usual reading.
     */
    function moveBadge(move) {
        const badge = el('span', {
            class: 'count-move',
            'data-dir': move > 0 ? 'up' : 'down',
            // The sign is in the text, not only in the colour: the badge has to say
            // which way it went to a reader who does not see the red or the green.
            text: (move > 0 ? '+' : '-') + group(Math.abs(move))
        });
        // Gone from the tree once it has faded, not merely transparent: a screen
        // reader would otherwise still announce a badge nobody can see any more.
        badge.addEventListener('animationend', function () {
            badge.remove();
        });
        return badge;
    }

    function actionRow(run, spec) {
        const row = el('div', {class: 'outcome-actions'}, [actionButton(spec.primary, true)]);
        if (spec.secondary) row.appendChild(actionButton(spec.secondary, false));
        if (spec.note) {
            const note = typeof spec.note === 'function' ? spec.note : null;
            row.appendChild(live(el('span', {class: 'outcome-note', text: note ? note() : spec.note}), note));
        }
        return row;
    }

    function actionButton(spec, primary) {
        const className = 'action' + (primary && spec.filled ? ' action-filled' : '')
            + (primary ? '' : ' action-secondary');
        if (spec.href) return el('a', {class: className, href: spec.href, text: spec.label});
        const button = el('button', {type: 'button', class: className, text: spec.label});
        button.addEventListener('click', function () {
            spec.action(button);
        });
        return button;
    }

    function resubmit(run, button) {
        fetch('/api/analyses', {
            method: 'POST',
            headers: {'content-type': 'application/json'},
            body: JSON.stringify({source_id: run.source_id, request: run.request || {}})
        }).then(signInOn401).then(function (response) {
            return response.json().then(function (payload) {
                return {ok: response.ok, payload: payload};
            });
        }).then(function (result) {
            if (!result.ok || !result.payload.id) {
                throw new Error(PSL.problemText(result.payload, undefined, t('run.resubmit_refused')));
            }
            location.hash = '#/run/' + result.payload.id;
        }).catch(function (error) {
            // Silence here reads as a broken button: the operator clicked and
            // nothing moved. The note carries the run's arguments, so the error
            // borrows the line and hands it back.
            const note = button.parentNode && button.parentNode.querySelector('.outcome-note');
            if (!note) return;
            // Stashed on the node, not in a closure: a second failure inside the
            // window would otherwise capture the first error as the text to restore
            // and pin it there for good.
            if (note.dataset.restore === undefined) note.dataset.restore = note.textContent;
            clearTimeout(state.noteTimer);
            note.textContent = String(error.message || error);
            note.setAttribute('data-error', 'true');
            state.noteTimer = setTimeout(function () {
                note.textContent = note.dataset.restore;
                delete note.dataset.restore;
                note.removeAttribute('data-error');
            }, 6000);
        });
    }

    function loadRun(id) {
        return getJson('/api/analyses/' + id).then(function (run) {
            state.run = run;
            state.runError = false;
            state.runReadFailures = 0;
            render();
            if (run.status === 'pending' || run.status === 'running') scheduleRunPoll(id, 1000);
        }).catch(function (error) {
            if (PSL.runReadOutcome(error) === 'unknown') {
                state.runError = id;
                state.runReadFailures = 0;
                render();
                return;
            }
            // The Hub restarting, or a proxy in front of it: what the page shows stays
            // true, so it keeps it and asks again until the Hub answers.
            state.runReadFailures = (state.runReadFailures || 0) + 1;
            render();
            scheduleRunPoll(id, PSL.retryDelay(state.runReadFailures));
        });
    }

    function scheduleRunPoll(id, delay) {
        clearTimeout(state.runTimer);
        state.runTimer = setTimeout(function () {
            if (currentRunId() === id) loadRun(id);
        }, delay);
    }

    /** Said while reads of this run fail for a reason other than its absence. */
    function runUnreachableBanner() {
        return el('div', {class: 'banner', 'data-tone': 'warn', role: 'status'}, [
            warningGlyph(16),
            el('p', {text: t('run.hub_down')})
        ]);
    }


    // ------------------------------------------------------- advanced: detection

    /**
     * One sentence per knob, saying what the detector stops seeing when the
     * number goes up. Written in the terms of what is looked for, never in terms
     * of file size: raising a threshold does not shorten a report, it decides
     * that a smaller pattern is no longer a problem.
     */
    const DETECTION_COPY = {
        n_plus_one_min_occurrences: 'detect.n_plus_one_min_occurrences',
        window_duration_ms: 'detect.window_duration_ms',
        slow_query_threshold_ms: 'detect.slow_query_threshold_ms',
        slow_query_min_occurrences: 'detect.slow_query_min_occurrences',
        max_fanout: 'detect.max_fanout',
        chatty_service_min_calls: 'detect.chatty_service_min_calls',
        pool_saturation_concurrent_threshold: 'detect.pool_saturation_concurrent_threshold',
        serialized_min_sequential: 'detect.serialized_min_sequential',
        sanitizer_aware_classification: 'detect.sanitizer_aware_classification',
        sanitizer_aware_min_cv: 'detect.sanitizer_aware_min_cv'
    };

    function detectionKnobs() {
        return (state.status && state.status.detection_knobs) || [];
    }

    function detectionCount() {
        return Object.keys(state.form.detection).length;
    }

    function setDetection(name, raw, knob) {
        // A choice stays the word it is, a threshold becomes the number it reads as.
        const value = knob.kind === 'choice' ? raw : Number(raw);
        const unreadable = knob.kind !== 'choice' && !Number.isFinite(value);
        // An empty field or the engine's own default is not an override: recording
        // it would make the run card claim a departure that never happened.
        if (raw === '' || unreadable || value === knob.default) delete state.form.detection[name];
        else state.form.detection[name] = value;
        updateSubmit();
        refreshDetectionCount();
    }

    function refreshDetectionCount() {
        const badge = document.getElementById('advanced-count');
        if (!badge) return;
        const count = detectionCount();
        badge.hidden = count === 0;
        badge.textContent = t(count === 1 ? 'daemon.settings.changed.one' : 'daemon.settings.changed.many', {count: count});
        const resetAll = document.getElementById('advanced-reset');
        // Nothing to put back when nothing was moved, and a button that does
        // nothing is a button that has to be tried to be understood.
        if (resetAll) resetAll.hidden = count === 0;
    }

    /**
     * A disclosure, and the only one in this form. It holds settings that
     * change what the analysis looks for, which is a different question from
     * every other control on this screen, so it is folded away rather than
     * mixed in.
     */
    function advancedPanel() {
        const knobs = detectionKnobs();
        if (knobs.length === 0) return null;

        const summary = el('summary', {class: 'advanced-summary'}, [
            warningGlyph(14),
            // The glyph is aria-hidden, so the caution it carries needs words a
            // screen reader receives while the panel is still collapsed.
            el('span', {class: 'visually-hidden', text: t('new.advanced.warning')}),
            el('span', {class: 'overline'}, [
                el('span', {class: 'over-warn', text: t('new.advanced.head')}),
                document.createTextNode(t('new.advanced.what'))
            ]),
            el('span', {id: 'advanced-count', class: 'advanced-count', hidden: 'hidden'})
        ]);

        const body = el('div', {class: 'advanced-body'}, [
            el('section', {class: 'notice-block', 'data-tone': 'warn'}, [
                warningGlyph(17),
                el('div', {class: 'notice-block-text'}, [
                    el('p', {class: 'notice-block-title', text: t('new.advanced.title')}),
                    el('p', {class: 'notice-block-body', text: t('new.advanced.body')})
                ])
            ]),
            el('p', {class: 'advanced-lead', text: t('new.advanced.lead')})
        ]);

        // One button for the lot, beside the count so the two agree at a glance.
        const resetAll = el('button', {
            type: 'button',
            id: 'advanced-reset',
            class: 'pill-button pill-sm advanced-reset',
            text: t('new.advanced.reset')
        });
        resetAll.addEventListener('click', function () {
            state.form.detection = {};
            updateSubmit();
            render();
        });
        body.appendChild(el('div', {class: 'advanced-actions'}, [resetAll]));

        knobs.forEach(function (knob) {
            body.appendChild(detectionRow(knob));
        });

        const panel = el('details', {class: 'advanced'}, [summary, body]);
        // Open because the reader left it open, or because a threshold in it is
        // set and hiding that would hide what the run is about to do.
        panel.open = state.panelOpen.advanced === true || detectionCount() > 0;
        panel.addEventListener('toggle', function () {
            // Only a change the reader made: every render rebuilds this panel and
            // sets `open` above, which fires this same event.
            if (state.panelOpen.advanced === panel.open) return;
            state.panelOpen.advanced = panel.open;
            saveFolds();
        });
        queueMicrotask(refreshDetectionCount);
        return panel;
    }

    function detectionRow(knob) {
        const current = state.form.detection[knob.name];
        const identifier = 'knob-' + knob.name;
        // The default as a value, not as a placeholder. Empty, the field had
        // nothing for the spinner to step from, so the up arrow jumped to the
        // minimum: 10 became 2. It stays in the muted tone until it is moved,
        // and a value equal to the default is still not an override.
        const shown = current === undefined ? String(knob.default) : String(current);
        const input = knob.kind === 'choice'
            ? el('select', {id: identifier, class: 'input input-knob'}, knob.choices.map(function (choice) {
                return el('option', {value: choice, text: choice});
            }))
            : el('input', {
                id: identifier,
                type: 'number',
                class: 'input input-knob',
                min: String(knob.min),
                max: String(knob.max),
                // A decimal steps by a hundredth, an integer by one.
                step: knob.kind === 'decimal' ? '0.01' : '1',
                value: shown
            });
        // A select takes its value once its options exist, not as an attribute.
        if (knob.kind === 'choice') input.value = shown;
        const label = t('new.knob.reset', {name: knob.name, value: knob.default});
        const reset = el('button', {
            type: 'button',
            class: 'knob-reset',
            // The glyph is aria-hidden like every other in the product, so the button
            // carries the words itself, and a title says them to a mouse as well.
            'aria-label': label,
            title: label
        }, [undoGlyph(14)]);

        function mark() {
            const moved = state.form.detection[knob.name] !== undefined;
            input.toggleAttribute('data-default', !moved);
            reset.hidden = !moved;
        }

        mark();

        input.addEventListener('input', function () {
            setDetection(knob.name, input.value, knob);
            mark();
        });
        reset.addEventListener('click', function () {
            input.value = String(knob.default);
            setDetection(knob.name, input.value, knob);
            mark();
        });

        return el('div', {class: 'knob'}, [
            el('label', {class: 'knob-head', for: identifier}, [
                el('span', {class: 'knob-name', text: knob.name}),
                el('span', {class: 'knob-default', text: t('daemon.settings.default', {value: knob.default})})
            ]),
            proseInto(el('span', {class: 'knob-body'}), DETECTION_COPY[knob.name] ? t(DETECTION_COPY[knob.name]) : ''),
            el('div', {class: 'knob-controls'}, [input, reset])
        ]);
    }

    // ------------------------------------------------------ screen: incidents

    const INCIDENT_PAGE = 100;
    // In the order the daemon's own monitor tab prints them, so the two
    // surfaces never disagree about what comes first. The TUI prints
    // ns/service in one cell, this table gives the namespace its own.
    const INCIDENT_COLUMNS = ['started', 'namespace', 'service', 'kind', 'ended', 'findings', 'capture', 'source'];
    const INCIDENT_COLUMNS_RIGHT = ['started', 'ended', 'findings'];
    /** The text key of what each capture verdict means. */
    const INCIDENT_CAPTURE = {
        complete: 'incidents.capture.complete',
        partial: 'incidents.capture.partial',
        empty: 'incidents.capture.empty'
    };

    /** The page size, held under the operator's read limit, which the Hub rejects above rather than clamps. */
    function incidentPage() {
        const limits = state.status && state.status.limits;
        return Math.min(INCIDENT_PAGE, (limits && limits.max_read_limit) || INCIDENT_PAGE);
    }

    /** The page, then every filter that is set, under the name the route takes it by. */
    function incidentsQuery(offset) {
        const filter = state.incidentFilter;
        return Object.keys(filter).reduce(function (query, key) {
            return filter[key] ? query + '&' + key + '=' + encodeURIComponent(filter[key]) : query;
        }, '?limit=' + incidentPage() + '&offset=' + offset);
    }

    function incidentsPath(offset) {
        return '/api/incidents' + incidentsQuery(offset);
    }

    /**
     * The first page for the current filter, read from the daemons themselves
     * rather than from whatever the last poll left behind: an operator paged
     * about an OOM kill opens this screen within the minute, and the poll runs
     * hourly. The route reads the fleet and answers the listing, so this is one
     * round trip, and its own floor per source keeps a reload loop off the
     * daemons. Older pages append from the store, see loadOlderIncidents.
     */
    function loadIncidents() {
        state.incidents = null;
        // A re-read may bring episodes pushed since: an unfolded incident asks again.
        state.incidentCorrelations = {};
        state.incidentsError = false;
        state.incidentsDone = false;
        state.incidentsReading = true;
        render();
        return postJson('/api/incidents/refresh' + incidentsQuery(0)).then(function (rows) {
            adoptIncidents(rows, []);
        }).catch(function () {
            // The fleet was not read: refused by the gate while two reads run,
            // or the route failed. The store still holds what the last read
            // left, and showing it beats an empty screen with a red banner.
            return getJson(incidentsPath(0)).then(function (rows) {
                state.incidentsError = 'stale';
                adoptIncidents(rows, []);
            }).catch(function () {
                state.incidentsError = 'unread';
                state.incidents = [];
            });
        }).finally(function () {
            state.incidentsReading = false;
            state.incidentsRead = true;
            // The sources carry the read stamps this screen prints under its
            // table, and the read that just ran moved them. Awaited, or the
            // stamps would be one render behind the rows they describe.
            return reloadSources().then(function () {
                if (currentScreen() === 'incidents') render();
            });
        });
    }

    function loadOlderIncidents(button) {
        button.disabled = true;
        getJson(incidentsPath(state.incidents.length)).then(function (rows) {
            state.incidentsError = false;
            adoptIncidents(rows, state.incidents);
        }).catch(function () {
            state.incidentsError = 'page';
        }).finally(function () {
            if (currentScreen() === 'incidents') render();
        });
    }

    /**
     * Pages are offsets into a table the polls keep growing at the top, so an
     * older page can repeat the row the first one ended on. A row already held
     * is dropped, the end of the listing is still read from the raw page, and
     * the next offset re-reads that boundary row, which this drops again.
     */
    function adoptIncidents(rows, previous) {
        const seen = new Set(previous.map(incidentKey));
        state.incidents = previous.concat(rows.filter(function (incident) {
            return !seen.has(incidentKey(incident));
        }));
        state.incidentsDone = rows.length < incidentPage();
        rows.forEach(function (incident) {
            if (state.incidentServices.indexOf(incident.service) < 0) state.incidentServices.push(incident.service);
            if (incident.namespace && state.incidentNamespaces.indexOf(incident.namespace) < 0) {
                state.incidentNamespaces.push(incident.namespace);
            }
        });
        state.incidentServices.sort();
        state.incidentNamespaces.sort();
    }

    function renderIncidentsScreen() {
        const section = el('section', {}, [
            ruledOverline(t('incidents.overline')),
            el('h1', {class: 'page-title', text: t('incidents.title')}),
            el('p', {class: 'page-sub'}, [
                document.createTextNode(t('incidents.sub')),
                helpDot(t('incidents.help'), t('incidents.topic'))
            ])
        ]);

        if (state.loading || state.incidents === null) {
            // The filter line comes first even here, so the read-now control is
            // on screen, and visibly dead, while the read it started runs.
            section.appendChild(incidentFilterLine());
            section.appendChild(el('div', {class: 'sources-wrap'}, [skeletonTable()]));
            return section;
        }
        if (state.incidentsError) {
            section.appendChild(el('div', {class: 'banner', 'data-tone': INCIDENT_ERROR_TONE[state.incidentsError]}, [
                state.incidentsError === 'stale' ? warningGlyph(16) : critGlyph(16),
                el('div', {text: t(INCIDENT_ERROR_TEXT[state.incidentsError])})
            ]));
            if (state.incidents.length === 0) {
                // The read-now control belongs on screen even here: without it
                // the only way out of a refused read is a page reload.
                section.appendChild(incidentFilterLine());
                return section;
            }
        }
        unauthorizedBanners().forEach(function (banner) {
            section.appendChild(banner);
        });
        section.appendChild(incidentFilterLine());
        if (state.incidents.length === 0) {
            section.appendChild(el('div', {class: 'empty-state'}, [
                el('p', {class: 'empty-title', text: t('incidents.empty.title')}),
                proseInto(el('p'), t(Object.values(state.incidentFilter).some(Boolean)
                    ? 'incidents.empty.body_filtered' : 'incidents.empty.body'))
            ]));
            appendCopyNote(section);
            return section;
        }
        section.appendChild(el('div', {class: 'sources-wrap'}, [incidentsTable(state.incidents)]));
        if (!state.incidentsDone) {
            const older = el('button', {type: 'button', class: 'pill-button'}, [
                svg([['path', {d: 'M12 5v14M5 12l7 7 7-7'}]], 14),
                el('span', {text: t('incidents.older')})
            ]);
            older.addEventListener('click', function () {
                loadOlderIncidents(older);
            });
            section.appendChild(el('p', {class: 'sources-note'}, [older]));
        }
        appendCopyNote(section);
        section.appendChild(el('p', {
            class: 'sources-note',
            text: t('incidents.note')
        }));
        return section;
    }

    /** One banner per daemon that refused the Hub's key on its incidents route. */
    function unauthorizedBanners() {
        return (state.sources || []).filter(function (source) {
            return source.incidents_state === 'unauthorized';
        }).map(function (source) {
            return el('div', {class: 'banner', 'data-tone': 'warn'}, [
                warningGlyph(16),
                el('div', {}, [
                    el('p', {}, [
                        el('span', {text: t('incidents.unauthorized.lead', {name: source.name})}),
                        el('span', {class: 'code-inline', text: '[daemon] read_api_key'}),
                        el('span', {text: t('incidents.unauthorized.goes')}),
                        el('span', {class: 'code-inline', text: 'AuthHeaderValue'}),
                        el('span', {text: t('incidents.unauthorized.sent_as')}),
                        el('span', {class: 'code-inline', text: 'X-API-Key'}),
                        el('span', {text: '.'})
                    ])
                ])
            ]);
        });
    }

    /**
     * One labelled select for the filter line, labelled by the text of `name`. The first
     * option is the whole set, and `options` are [value, text] pairs.
     */
    function filterSelect(name, value, options, onChange, disabled) {
        const label = t('filter.' + name);
        const select = el('select', {
            class: 'refresh-select', 'aria-label': label, disabled: disabled ? 'disabled' : null
        }, [
            el('option', {value: '', text: t('filter.' + name + '.every')})
        ].concat(options.map(function (entry) {
            const option = el('option', {value: entry[0], text: entry[1]});
            if (entry[0] === value) option.selected = true;
            return option;
        })));
        select.addEventListener('change', function () {
            onChange(select.value);
        });
        // A fragment, so the label and the select land as siblings in the line
        // and share its gap, the way the read-now button beside them does.
        const pair = document.createDocumentFragment();
        pair.appendChild(el('span', {class: 'refresh-label', text: label}));
        pair.appendChild(select);
        return pair;
    }

    /** The change handler of one filter: set it, then read the fleet again under it. */
    function setFilter(key) {
        return function (value) {
            state.incidentFilter[key] = value;
            loadIncidents();
        };
    }

    function incidentFilterLine() {
        const filter = state.incidentFilter;
        const same = function (value) {
            return [value, value];
        };
        const daemons = (state.sources || []).filter(function (source) {
            return source.kind === 'daemon';
        });
        const filters = PSL.incidentFilterState(state.incidentsRead, state.incidentNamespaces);
        const environments = Array.from(new Set(daemons.map(function (source) {
            return source.environment;
        }).filter(Boolean))).sort();
        const read = el('button', {
            type: 'button',
            class: 'pill-button',
            // Disabled rather than queued: a second read would be refused by the
            // route's own floor anyway, and a dead button says why better.
            disabled: state.incidentsReading ? 'disabled' : null
        }, [
            svg([['path', {d: 'M20 11a8 8 0 1 0-2.3 5.7'}], ['path', {d: 'M20 5v6h-6'}]], 14),
            el('span', {text: t(state.incidentsReading ? 'incidents.reading' : 'incidents.read_now')})
        ]);
        read.addEventListener('click', function () {
            loadIncidents();
        });
        // Kind and service answer most questions. The other three fold away
        // behind a toggle that says how many of them are set.
        const more = el('button', {
            type: 'button', class: 'pill-button', id: 'more-filters',
            'aria-expanded': state.incidentFiltersOpen ? 'true' : 'false', 'aria-controls': 'more-filter-fields'
        }, [
            svg([['path', {d: state.incidentFiltersOpen ? 'M6 15l6-6 6 6' : 'M6 9l6 6 6-6'}]], 12),
            el('span', {text: PSL.moreFiltersLabel(filter)})
        ]);
        more.addEventListener('click', function () {
            state.incidentFiltersOpen = !state.incidentFiltersOpen;
            renderFocus('#more-filters');
        });
        const folded = el('div', {class: 'refresh refresh-more', id: 'more-filter-fields'}, [
            filters.namespace
                ? filterSelect('namespace', filter.namespace, state.incidentNamespaces.map(same),
                    setFilter('namespace'), filters.pending)
                : null,
            filterSelect('environment', filter.environment, environments.map(same), setFilter('environment')),
            filterSelect('daemon', filter.source_id, daemons.map(function (source) {
                return [source.id, source.name];
            }), setFilter('source_id'))
        ]);
        folded.hidden = !state.incidentFiltersOpen;
        return el('div', {class: 'filter-stack'}, [
            el('div', {class: 'refresh'}, [
                filterSelect('kind', filter.kind, Object.keys(PSL.INCIDENT_KIND_LABEL).map(function (kind) {
                    return [kind, PSL.INCIDENT_KIND_LABEL[kind]];
                }), setFilter('kind')),
                // Service lists what the read returns, so it waits for it rather
                // than open on "every" alone and change under the pointer.
                filterSelect('service', filter.service, state.incidentServices.map(same), setFilter('service'),
                    filters.pending),
                more,
                read
            ]),
            folded
        ]);
    }

    // Three ways the screen can be short of a reading, and they are not the
    // same news. `stale` is the Hub answering promptly that it is already
    // running two fleet reads, or a read that failed, with the stored copy
    // still on screen. `unread` is the Hub not answering at all. `page` is one
    // older page that did not append, the rows above it untouched.
    const INCIDENT_ERROR_TONE = {stale: 'warn', unread: 'crit', page: 'crit'};
    const INCIDENT_ERROR_TEXT = {
        stale: 'incidents.error.stale',
        unread: 'incidents.error.unread',
        page: 'incidents.error.page'
    };

    /**
     * One line per daemon, saying when the Hub last read its ring. Without it a
     * quiet fleet and a stale copy read the same, which is the whole reason the
     * screen reads on open.
     */
    function incidentCopyNote() {
        const daemons = (state.sources || []).filter(function (source) {
            return source.kind === 'daemon';
        });
        if (daemons.length === 0) return null;
        return el('p', {class: 'sources-note'}, [
            el('span', {text: t('incidents.copy_note')})
        ].concat(daemons.map(function (source) {
            const text = function () {
                return PSL.incidentsCopy(source, Date.now());
            };
            // The title on the wrapper, not on the live span: the ticker copies
            // its own text into any title it finds on the node it rewrites.
            return el('span', {
                title: source.incidents_read_ms == null ? null : PSL.dateTime(source.incidents_read_ms)
            }, [
                live(el('span', {text: text()}), text),
                el('span', {text: '. '})
            ]);
        })));
    }

    function appendCopyNote(section) {
        const note = incidentCopyNote();
        if (note) section.appendChild(note);
    }

    function incidentsTable(incidents) {
        const head = el('tr', {}, INCIDENT_COLUMNS.map(function (name) {
            return el('th', {
                text: t('incidents.col.' + name),
                scope: 'col',
                'data-align': INCIDENT_COLUMNS_RIGHT.indexOf(name) >= 0 ? 'right' : null
            });
        }));
        return el('table', {class: 'table'}, [
            el('thead', {}, [head]),
            el('tbody', {}, incidents.flatMap(incidentRow))
        ]);
    }

    function incidentRow(incident) {
        const capture = PSL.incidentCapture(incident);
        const row = el('tr', {});
        // The title on the cell, not on the live span: the ticker copies its
        // text into any title it finds on the node it rewrites.
        row.appendChild(el('td', {'data-align': 'right', title: PSL.dateTime(incident.at_ms)}, [
            live(el('span', {text: startedText(incident)}), function () {
                return startedText(incident);
            })
        ]));
        row.appendChild(el('td', {class: 'table-mono', text: incident.namespace || ''}));
        row.appendChild(incidentNameCell(incident));
        row.appendChild(el('td', {}, [el('span', {
            class: 'chip',
            'data-kind': incident.kind,
            text: PSL.INCIDENT_KIND_LABEL[incident.kind] || incident.kind,
            title: incident.detail || null
        })]));
        row.appendChild(incident.ended_at_ms
            ? el('td', {
                'data-align': 'right',
                text: t('incidents.ended_after', {duration: PSL.dur(incident.ended_at_ms - incident.at_ms)}),
                title: PSL.dateTime(incident.ended_at_ms)
            })
            : el('td', {'data-align': 'right'}, [el('span', {class: 'table-muted', text: t('incidents.still_open')})]));
        row.appendChild(el('td', {'data-align': 'right', text: String(incident.finding_count)}));
        row.appendChild(el('td', {title: t(INCIDENT_CAPTURE[capture])}, [
            el('span', {class: capture === 'empty' ? 'table-muted' : null, text: t('incidents.capture_word.' + capture)})
        ]));
        row.appendChild(el('td', {}, [
            el('span', {class: 'table-strong', text: incident.source_name || incident.source_id}),
            el('span', {text: ' '}),
            el('span', {class: 'chip chip-declared', text: incident.environment || t('incidents.unknown')})
        ]));

        const cell = el('td', {
            id: 'incident-detail-' + incidentKey(incident),
            colspan: String(INCIDENT_COLUMNS.length)
        }, [incidentPanel(incident)]);
        const detail = el('tr', {class: 'daemon-detail'}, [cell]);
        detail.hidden = state.panelOpen[foldKey(incident)] !== true;
        if (!detail.hidden && state.incidentDetails[incident.id] === undefined) {
            queueMicrotask(function () {
                loadIncident(incident);
            });
        }
        return [row, detail];
    }

    function startedText(incident) {
        return t('sources.ago', {duration: PSL.dur(Date.now() - incident.at_ms)});
    }

    /** A row is one daemon's capture: two sources fed the same alert list the same id, once each. */
    function incidentKey(incident) {
        return incident.id + '-' + incident.source_id;
    }

    function foldKey(incident) {
        return 'incident:' + incidentKey(incident);
    }

    function incidentNameCell(incident) {
        const button = el('button', {
            type: 'button',
            class: 'row-toggle',
            'aria-expanded': state.panelOpen[foldKey(incident)] === true ? 'true' : 'false',
            'aria-controls': 'incident-detail-' + incidentKey(incident),
            // Several incidents share a service: the kind and the age tell their buttons apart.
            'aria-label': (PSL.INCIDENT_KIND_LABEL[incident.kind] || incident.kind) + ', ' + incident.service
                + ', ' + startedText(incident)
        }, [el('span', {text: incident.service})]);
        button.addEventListener('click', function () {
            toggleIncident(incident, button);
        });
        return el('td', {class: 'table-strong'}, [button]);
    }

    /** Folded in place, like a daemon row, and fetched once: a frozen record does not move. */
    function toggleIncident(incident, button) {
        const open = button.getAttribute('aria-expanded') !== 'true';
        button.setAttribute('aria-expanded', open ? 'true' : 'false');
        state.panelOpen[foldKey(incident)] = open;
        saveFolds();
        const cell = document.getElementById('incident-detail-' + incidentKey(incident));
        if (cell) cell.parentNode.hidden = !open;
        const detail = state.incidentDetails[incident.id];
        if (open && (detail === undefined || (detail !== 'loading' && detail.error_code))) loadIncident(incident);
    }

    function loadIncident(incident) {
        state.incidentDetails[incident.id] = 'loading';
        const cell = document.getElementById('incident-detail-' + incidentKey(incident));
        if (cell) cell.replaceChildren(incidentPanel(incident));
        getJson('/api/incidents/' + encodeURIComponent(incident.id))
            .then(function (record) {
                state.incidentDetails[incident.id] = record;
            })
            .catch(function (error) {
                state.incidentDetails[incident.id] = {
                    error_code: /answered 404$/.test(String(error && error.message)) ? 'gone' : 'internal'
                };
            })
            .finally(function () {
                const target = document.getElementById('incident-detail-' + incidentKey(incident));
                if (target) target.replaceChildren(incidentPanel(incident));
            });
    }

    function incidentPanel(incident) {
        const detail = state.incidentDetails[incident.id];
        if (detail === 'loading' || detail === undefined) {
            return el('div', {class: 'daemon-panel'}, [
                el('p', {class: 'daemon-loading', role: 'status', text: t('incidents.panel.reading')}),
                el('div', {class: 'skeleton', style: 'height:90px'})
            ]);
        }
        if (detail.error_code) {
            return el('div', {class: 'daemon-panel'}, [
                el('div', {class: 'banner', 'data-tone': 'crit'}, [
                    critGlyph(16),
                    el('p', {
                        text: t(detail.error_code === 'gone' ? 'incidents.panel.gone' : 'incidents.panel.failed')
                    })
                ])
            ]);
        }
        const capture = PSL.incidentCapture(detail);
        const findings = PSL.sortFindings(detail.findings || [], state.findingSort.key, state.findingSort.dir);
        // A link, so it opens in a new tab too. Its end is clamped to the moment it is followed.
        const analyse = el('a', {class: 'pill-button', href: PSL.incidentHandoffHash(detail, Date.now())}, [
            svg([['path', {d: 'M5 12h14M13 6l6 6-6 6'}]], 14),
            el('span', {text: t('incidents.analyse')})
        ]);
        // The button comes after the note that describes the window, since it
        // is the action the note argues for. A bare row, so the panel's own
        // gap sets the distance and no margin adds a second one.
        return el('div', {class: 'daemon-panel'}, [
            el('p', {class: 'overline daemon-audience', text: t('incidents.panel.overline')}),
            el('p', {
                class: 'daemon-source-note',
                text: t(INCIDENT_CAPTURE[capture]) + ' ' + t('incidents.panel.window', {
                    before: PSL.dur(detail.at_ms - detail.window_from_ms),
                    after: PSL.dur(detail.window_to_ms - detail.at_ms)
                })
            }),
            el('div', {}, [analyse]),
            findings.length === 0
                ? el('p', {class: 'daemon-lead', text: t('incidents.panel.no_finding')})
                : findingsFilter(detail),
            findings.length === 0 ? null : el('div', {class: 'sources-wrap'}, [incidentFindingsTable(findings, detail)]),
            incidentCorrelationsBlock(incident)
        ]);
    }

    /** Narrows the frozen findings in place, on their endpoint or template, without drawing the screen again. */
    function findingsFilter(incident) {
        const key = incidentKey(incident);
        const input = el('input', {
            type: 'search', class: 'input findings-filter', placeholder: t('incidents.filter.placeholder'),
            value: state.findingQuery[key] || '', autocomplete: 'off', spellcheck: 'false'
        });
        const count = el('span', {class: 'field-gloss', 'aria-live': 'polite'});
        const apply = function () {
            state.findingQuery[key] = input.value;
            const rows = document.querySelectorAll(`[data-findings="${CSS.escape(key)}"] tbody tr`);
            let shown = 0;
            Array.prototype.forEach.call(rows, function (row) {
                row.hidden = !PSL.findingMatches(FINDING_ROW.get(row), input.value);
                if (!row.hidden) shown += 1;
            });
            count.textContent = input.value.trim() === '' ? '' : t('incidents.filter.shown', {shown: shown, total: rows.length});
        };
        input.addEventListener('input', apply);
        // Once the table beside it is in the page, so a query kept across a sort applies again.
        queueMicrotask(apply);
        const row = field(t('incidents.filter.label'), input, null);
        row.appendChild(count);
        return row;
    }

    /** A severity as a chip reads it: the engine's three in the shown language, any other as sent. */
    function severityLabel(severity) {
        return severity && PSL.hasText('severity.' + severity) ? t('severity.' + severity) : severity || '?';
    }

    // Each table row's frozen finding, for the filter to test without reading it back from the cells.
    const FINDING_ROW = new WeakMap();

    function incidentFindingsTable(findings, incident) {
        // Without a single ack credential in the Hub, every link would open a page that can do nothing.
        const canAck = (state.sources || []).some(function (source) {
            return source.ack_relay;
        });
        const columns = ['type', 'severity', 'impact', 'endpoint', 'template', 'seen', 'first'].concat(canAck ? ['ack'] : []);
        const head = el('tr', {}, columns.map(function (name) {
            return findingHead(name, incident);
        }));
        const rows = findings.map(function (row) {
            const finding = row.finding || {};
            const phase = PSL.findingPhase(row, incident);
            const template = PSL.findingTemplate(row);
            const impact = PSL.findingImpact(row);
            const tr = el('tr', {}, [
                // Not table-mono: this reads as a label now, not as the
                // identifier the API filters on, which is untouched.
                el('td', {text: PSL.FINDING_TYPE_LABEL[finding.type] || finding.type || '?'}),
                el('td', {}, [el('span', {
                    class: 'chip',
                    'data-sev': finding.severity || null,
                    text: severityLabel(finding.severity)
                })]),
                el('td', {'data-align': 'right', text: impact == null ? '—' : PSL.num(impact)}),
                el('td', {class: 'table-mono', text: finding.source_endpoint || '?'}),
                // Whole, wrapped: the template is what tells two rows apart.
                el('td', {class: 'table-mono'}, [el('span', {class: 'findings-template', text: template || '—'})]),
                el('td', {'data-align': 'right', text: String(row.seen_count == null ? '?' : row.seen_count)}),
                el('td', {
                    title: PSL.dateTime(row.first_seen_ms),
                    text: PSL.dur(Math.abs(row.first_seen_ms - incident.at_ms)) + PSL.phaseLabel(phase, incident.kind)
                }),
                // The daemon that froze the finding is the only one the page checks.
                canAck ? el('td', {}, [finding.signature ? ackLink(finding, incident) : null]) : null
            ]);
            FINDING_ROW.set(tr, row);
            return tr;
        });
        const table = el('table', {class: 'table table-findings', 'data-findings': incidentKey(incident)},
            [el('thead', {}, [head]), el('tbody', {}, rows)]);
        if (canAck) return table;
        return el('div', {}, [table, el('p', {
            class: 'field-note',
            text: t('incidents.findings.no_credential')
        })]);
    }

    /** The columns a click sorts on, each under the sort key of its name. The ack column does not sort. */
    const FINDING_SORT_KEY = ['type', 'severity', 'impact', 'endpoint', 'template', 'seen', 'first'];

    /**
     * A column head that sorts on click. Counts and severity read worst first, the rest A to Z.
     * The impact head keeps the findings dashboard's own name, so the two read the same number the same way.
     */
    function findingHead(name, incident) {
        const key = FINDING_SORT_KEY.indexOf(name) >= 0 ? name : null;
        const label = t('incidents.finding.col.' + name);
        const th = el('th', {scope: 'col', 'data-align': name === 'seen' || name === 'impact' ? 'right' : null});
        if (!key) {
            th.textContent = label;
            return th;
        }
        const sort = state.findingSort;
        const active = sort.key === key;
        if (active) th.setAttribute('aria-sort', sort.dir === 'asc' ? 'ascending' : 'descending');
        const id = incidentKey(incident) + ':' + key;
        const button = el('button', {type: 'button', class: 'sort-head', 'data-sort': id}, [
            el('span', {text: label}),
            // Idle heads carry both arrows, or only the sorted column would read as sortable.
            svg([['path', {d: active ? (sort.dir === 'asc' ? 'M6 15l6-6 6 6' : 'M6 9l6 6 6-6') : 'M8 9l4-4 4 4M8 15l4 4 4-4'}]], 11)
        ]);
        button.addEventListener('click', function () {
            state.findingSort = active
                ? {key: key, dir: sort.dir === 'asc' ? 'desc' : 'asc'}
                : {key: key, dir: key === 'severity' || key === 'impact' || key === 'seen' ? 'desc' : 'asc'};
            // The ids come from the daemon: escaped, so a quote in one cannot break the selector.
            renderFocus('[data-sort="' + CSS.escape(id) + '"]');
        });
        th.appendChild(button);
        return th;
    }

    function ackLink(finding, incident) {
        const label = PSL.FINDING_TYPE_LABEL[finding.type] || finding.type || t('incidents.ack.finding');
        const link = el('a', {
            href: PSL.ackRouteHash(finding.signature, incident.source_id),
            class: 'ack-link',
            'aria-label': t('incidents.ack.label',
                {label: label, endpoint: finding.source_endpoint || t('incidents.ack.unknown_endpoint')}),
            text: t('incidents.ack.link')
        });
        link.addEventListener('click', function () {
            state.ackReturn = location.hash;
        });
        return link;
    }

    // ---------------------------------------------------- screen: correlations

    const CORRELATION_LIMIT = 1000;
    const CORRELATION_COLUMNS = ['pair', 'episodes', 'last', 'peak', 'lag', 'source'];
    const CORRELATION_COLUMNS_RIGHT = ['episodes', 'peak', 'lag'];
    const EPISODE_COLUMNS = ['episode', 'lasted', 'peak_conf', 'peak_co', 'incidents', ''];
    const EPISODE_COLUMNS_RIGHT = ['lasted', 'peak_conf', 'peak_co'];
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
            ruledOverline(t('correlations.overline')),
            el('h1', {class: 'page-title', text: t('correlations.title')}),
            el('p', {class: 'page-sub'}, [
                document.createTextNode(t('correlations.sub')),
                helpDot(t('correlations.help'), t('correlations.topic'))
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
                el('div', {text: t('correlations.error')})
            ]));
            return section;
        }
        const pairs = PSL.correlationPairs(state.correlations);
        if (pairs.length === 0) {
            section.appendChild(el('div', {class: 'empty-state'}, [
                el('p', {class: 'empty-title', text: t('correlations.empty.title')}),
                proseInto(el('p'), t('correlations.empty.body'))
            ]));
            return section;
        }
        section.appendChild(el('div', {class: 'sources-wrap'}, [correlationsTable(pairs)]));
        if (state.correlations.length === CORRELATION_LIMIT) {
            section.appendChild(el('p', {
                class: 'sources-note',
                text: t('correlations.limit', {limit: CORRELATION_LIMIT})
            }));
        }
        section.appendChild(el('p', {class: 'sources-note'}, [
            el('span', {text: t('correlations.note.lead')}),
            el('span', {class: 'code-inline', text: '[daemon.hub_export]'}),
            el('span', {text: t('correlations.note.and')}),
            el('span', {class: 'code-inline', text: '[daemon.correlation]'}),
            el('span', {text: t('correlations.note.tail')})
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
        const range = el('select', {class: 'refresh-select', 'aria-label': t('correlations.range')},
            PSL.CORRELATION_RANGES.map(function (value) {
                const option = el('option', {value: value, text: t('correlations.range.option', {span: PSL.humanDur(value)})});
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
            ? el('a', {class: 'pill-button pill-sm', href: '#/correlations'}, [el('span', {text: t('correlations.every_pair')})])
            : null;
        return el('div', {class: 'refresh'}, [
            pairOnly,
            el('span', {class: 'refresh-label', text: t('correlations.range')}),
            range,
            filterSelect('service', filter.service, state.correlationServices.map(function (service) {
                return [service, service];
            }), set('service')),
            filterSelect('daemon', filter.source_id, daemons.map(function (source) {
                return [source.id, source.name];
            }), set('source_id')),
            checkbox(filter.active, t('correlations.active_only'), set('active'))
        ]);
    }

    function correlationsTable(pairs) {
        const head = el('tr', {}, CORRELATION_COLUMNS.map(function (name) {
            return el('th', {
                text: t('correlations.col.' + name), scope: 'col', 'data-align': CORRELATION_COLUMNS_RIGHT.indexOf(name) >= 0 ? 'right' : null
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
            'aria-label': PSL.pairLabel(pair.last)
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
            el('td', {'data-align': 'right', class: 'table-mono', text: PSL.decimal(pair.peak.toFixed(2))}),
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
        return el('span', {class: 'pair'}, [pairSide(episode.source, null), pairSide(episode.target, t('correlations.then'))]);
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
            title: episode.ended_by === 'stale' ? t('correlations.stale_title') : null
        });
        if (episode.ended_at_ms == null) {
            cell.appendChild(document.createTextNode(PSL.dateTime(episode.started_at_ms) + ' → '));
            cell.appendChild(el('span', {class: 'chip', 'data-state': 'active', text: t('correlations.active')}));
        } else {
            cell.appendChild(document.createTextNode(PSL.episodeSpan(episode.started_at_ms, episode.ended_at_ms)));
            const note = PSL.episodeEndNote(episode);
            if (note) cell.appendChild(el('span', {class: 'table-muted', text: ' (' + note + ')'}));
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
        strip.setAttribute('aria-label', t('correlations.strip_label', {
            episodes: episodes.length, incidents: incidents.length, span: PSL.humanDur(state.correlationFilter.range)
        }));
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
                el('span', {text: PSL.dateTime(fromMs)}),
                el('span', {text: t('correlations.now')})
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
                text: name ? t('correlations.col.' + name) : '', scope: 'col', 'data-align': EPISODE_COLUMNS_RIGHT.indexOf(name) >= 0 ? 'right' : null
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
                el('td', {'data-align': 'right', class: 'table-mono', text: PSL.decimal(episode.peak_confidence.toFixed(2))}),
                el('td', {'data-align': 'right', class: 'table-mono', text: String(episode.peak_co_occurrences)}),
                el('td', {}, overlapped.length === 0
                    ? [el('span', {class: 'table-muted', text: t('correlations.no_incident')})]
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
                [el('span', {text: t('incidents.analyse')})]),
            traces ? el('a', {class: 'pill-button pill-sm', href: traces},
                [el('span', {text: t('correlations.explain')})]) : null
        ]);
    }

    /** An incident an episode overlaps, as a link that opens it unfolded on the incidents screen. */
    function incidentChip(incident) {
        const link = el('a', {
            class: 'chip',
            'data-kind': incident.kind,
            href: '#/incidents',
            text: (PSL.INCIDENT_KIND_LABEL[incident.kind] || incident.kind) + ' · ' + incident.service + ' · '
                + PSL.dateTime(incident.at_ms)
        });
        link.addEventListener('click', function () {
            state.incidentFilter = {
                service: '',
                namespace: '',
                kind: '',
                environment: '',
                source_id: incident.source_id
            };
            state.panelOpen[foldKey(incident)] = true;
            saveFolds();
        });
        return link;
    }

    /**
     * The episodes of an incident's daemon active during it, read once per unfolded incident.
     * An empty read asks once more whether the daemon pushes any at all, so a daemon without
     * the push never reads as a quiet one.
     */
    function loadIncidentCorrelations(incident) {
        const key = incidentKey(incident);
        const cacheKey = PSL.correlationWindowKey(key, incident.window_from_ms, incidentEnd(incident));
        state.incidentCorrelations[cacheKey] = 'loading';
        const scope = '/api/correlations?source_id=' + encodeURIComponent(incident.source_id);
        getJson(scope + '&from=' + incident.window_from_ms + '&to=' + incidentEnd(incident))
            .then(function (rows) {
                if (rows.length > 0) return {rows: rows, pushes: true};
                return getJson(scope + '&limit=1').then(function (any) {
                    return {rows: rows, pushes: any.length > 0};
                });
            })
            .then(function (result) {
                state.incidentCorrelations[cacheKey] = result;
            })
            .catch(function () {
                state.incidentCorrelations[cacheKey] = 'error';
            })
            .finally(function () {
                const cell = document.getElementById('incident-detail-' + key);
                if (cell) cell.replaceChildren(incidentPanel(incident));
            });
    }

    function incidentCorrelationsBlock(incident) {
        const cacheKey = PSL.correlationWindowKey(incidentKey(incident), incident.window_from_ms, incidentEnd(incident));
        const held = state.incidentCorrelations[cacheKey];
        if (held === undefined) {
            queueMicrotask(function () {
                loadIncidentCorrelations(incident);
            });
        }
        const block = el('div', {class: 'incident-correlations'}, [
            el('p', {class: 'overline daemon-audience', text: t('correlations.overline')}),
            el('h3', {class: 'daemon-lead', text: t('correlations.incident.title')}),
            el('p', {class: 'daemon-source-note', text: t('correlations.incident.sub')})
        ]);
        if (held === undefined || held === 'loading') {
            block.appendChild(el('p', {class: 'daemon-loading', role: 'status', text: t('correlations.incident.reading')}));
        } else if (held === 'error') {
            // Shown once: the next render of this panel, a refold or a re-read, asks again.
            delete state.incidentCorrelations[cacheKey];
            block.appendChild(el('p', {
                class: 'table-muted',
                text: t('correlations.incident.error')
            }));
        } else if (held.rows.length > 0) {
            block.appendChild(el('div', {class: 'sources-wrap'}, [incidentEpisodesTable(held.rows)]));
        } else {
            block.appendChild(el('p', {
                class: 'empty-state',
                text: held.pushes
                    ? t('correlations.incident.none')
                    : t('correlations.incident.no_push', {name: incident.source_name || incident.source_id})
            }));
        }
        return block;
    }

    function incidentEpisodesTable(episodes) {
        const head = el('tr', {}, ['pair', 'episode', 'peak', ''].map(function (name) {
            return el('th', {
                text: name ? t('correlations.col.' + name) : '', scope: 'col', 'data-align': name === 'peak' ? 'right' : null
            });
        }));
        const rows = episodes.map(function (episode) {
            return el('tr', {}, [
                el('td', {}, [pairCell(episode)]),
                episodeSpanCell(episode),
                el('td', {'data-align': 'right', class: 'table-mono', text: PSL.decimal(episode.peak_confidence.toFixed(2))}),
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
        }, [el('span', {text: t('correlations.all_episodes')})]);
    }

    // ------------------------------------------------- screen: ack one finding

    const ACK_WINDOW_DAYS = 7;

    /**
     * Reads the finding the link names, and the sources again, since they say
     * which daemons relay and how much each one's ack listing is worth. After a
     * submit the form and its result lines stay and the screen is not blanked.
     */
    function loadAck(afterSubmit) {
        const route = PSL.readAckRoute(location.hash);
        if (!route) return;
        if (!afterSubmit) {
            state.ack = {signature: route.signature, reason: '', expiry: '', lines: [], checked: {}};
            render();
        }
        const ack = state.ack;
        const path = '/api/findings?signature=' + encodeURIComponent(route.signature)
            + '&include_acked=true&limit=1';
        // Matched again here: the page must never offer a finding its link did
        // not name, whatever the Hub made of the filter.
        const named = function (rows) {
            return rows.find(function (row) {
                return row.finding && row.finding.signature === route.signature;
            }) || null;
        };
        // A second, windowed read adds the detections. Any failure of it, a refused read or
        // a body that is not a list, must not hide the card.
        const span = PSL.detectionWindow(Date.now(), ACK_WINDOW_DAYS);
        const windowed = getJson('/api/findings?signature=' + encodeURIComponent(route.signature)
            + '&from=' + span.from + '&to=' + span.to + '&limit=1').then(named).catch(function () {
            return undefined;
        });
        Promise.all([getJson(path), reloadSources(), windowed]).then(function (answers) {
            ack.finding = named(answers[0]);
            ack.volume = answers[2];
            ack.error = false;
        }).catch(function () {
            ack.error = true;
        }).finally(function () {
            // The reader's ticks stay: a source they left out is not checked again.
            ack.busy = false;
            if (state.ack === ack && currentScreen() === 'ack') render();
        });
    }

    function renderAckScreen() {
        const route = PSL.readAckRoute(location.hash);
        const section = el('section', {class: 'ack-stack'}, [el('div', {}, [
            el('a', {class: 'back-pill', href: state.ackReturn || '#/incidents'}, [
                svg([['path', {d: 'M15 18l-6-6 6-6'}]], 13),
                el('span', {text: t(state.ackReturn ? 'ack.back' : 'shell.tab.incidents')})
            ]),
            ruledOverline(t('ack.overline')),
            el('h1', {class: 'page-title', text: t('ack.title')}),
            el('p', {class: 'page-sub', text: t('ack.sub')})
        ])]);
        if (!route) {
            section.appendChild(el('div', {class: 'empty-state'}, [
                el('p', {class: 'empty-title', text: t('ack.incomplete.title')}),
                el('p', {text: t('ack.incomplete.body')})
            ]));
            return section;
        }
        const ack = state.ack;
        if (!ack || ack.signature !== route.signature || (ack.finding === undefined && !ack.error)) {
            section.appendChild(el('div', {class: 'card skeleton', style: 'height:220px'}));
            return section;
        }
        // First, and whatever the reload came to: these say what was written.
        if (ack.lines.length > 0) section.appendChild(ackResultLines(ack.lines));
        if (ack.error || !state.sources) {
            section.appendChild(el('div', {class: 'banner', 'data-tone': 'crit'}, [
                critGlyph(16),
                el('p', {text: t('ack.read_failed')})
            ]));
            return section;
        }
        if (!ack.finding) {
            section.appendChild(el('div', {class: 'empty-state'}, [
                el('p', {class: 'empty-title', text: t('ack.missing.title')}),
                el('p', {}, [
                    el('span', {text: t('ack.missing.body')}),
                    el('code', {class: 'code-inline', text: route.signature})
                ])
            ]));
            return section;
        }
        section.appendChild(ackFindingCard(ack.finding, ack.volume));
        // The route is the scope: the source or the environment it names sets the default ticks.
        section.appendChild(ackForm(PSL.ackRows(ack.finding, state.sources, route)));
        return section;
    }

    function ackResultLines(lines) {
        return el('div', {class: 'ack-stack', role: 'status'}, lines.map(function (line) {
            return el('div', {class: 'banner', 'data-tone': line.ok ? 'ok' : 'crit'}, [el('p', {text: line.text})]);
        }));
    }

    /**
     * The detections of the last seven days, from the windowed read: its
     * finding, null when the window holds no detection the Hub knows, undefined
     * when the read failed.
     */
    function ackDetectionFacts(volume) {
        if (!volume) return [];
        const facts = [[t('ack.fact.detections'), PSL.formatDetections(volume.detections, volume.detections_complete)]];
        if (volume.avoidable_io_ops != null) {
            facts.push([t('ack.fact.avoidable_io'),
                PSL.formatDetections(volume.avoidable_io_ops, volume.detections_complete)]);
        }
        return facts;
    }

    /** One line per source with its count, and what keeps it short. */
    function ackDetectionLines(volume) {
        if (volume === undefined) {
            return [el('p', {class: 'field-note', text: t('ack.detections.failed')})];
        }
        if (volume === null) {
            return [el('p', {class: 'field-note', text: t('ack.detections.none')})];
        }
        return (volume.sources || []).map(function (source) {
            const gaps = (source.detections_gaps || []).map(PSL.gapLabel);
            const io = source.avoidable_io_ops == null ? ''
                : t('ack.detections.io', {count: PSL.formatDetections(source.avoidable_io_ops, source.detections_complete)});
            return el('p', {
                class: 'field-note',
                text: t('ack.detections.line', {
                    name: source.name,
                    count: PSL.formatDetections(source.detections, source.detections_complete),
                    io: io,
                    gaps: gaps.length > 0 ? ' (' + gaps.join(', ') + ')' : ''
                })
            });
        });
    }

    function ackFindingCard(envelope, volume) {
        const finding = envelope.finding || {};
        const template = (finding.pattern || {}).template;
        const facts = [
            [t('ack.fact.service'), finding.service || '?'],
            [t('ack.fact.endpoint'), finding.source_endpoint || '?'],
            [t('ack.fact.first_seen'), PSL.dateTime(envelope.first_seen)],
            [t('ack.fact.last_seen'), PSL.dateTime(envelope.last_seen)],
            // The API's enum, worded: not_observed reads "not observed". A newer value shows as sent.
            [t('ack.fact.status'), PSL.hasText('ack.status.' + envelope.status) ? t('ack.status.' + envelope.status)
                : String(envelope.status || '?').replace(/_/g, ' ')]
        ].concat(ackDetectionFacts(volume));
        return el('div', {class: 'card params-panel'}, [
            el('div', {class: 'run-card-line'}, [
                el('span', {class: 'run-card-name', text: PSL.FINDING_TYPE_LABEL[finding.type] || finding.type || '?'}),
                el('span', {class: 'chip', 'data-sev': finding.severity || null, text: severityLabel(finding.severity)})
            ]),
            template ? el('span', {class: 'run-card-args', text: template, title: template}) : null,
            el('div', {class: 'run-card-facts'}, facts.map(function (fact) {
                return el('span', {class: 'fact'}, [
                    el('span', {class: 'fact-k', text: fact[0]}),
                    el('span', {class: 'fact-v', text: fact[1], title: fact[1]})
                ]);
            }))
        ].concat(ackDetectionLines(volume)));
    }

    /** What the two buttons would do with the form as it stands. */
    function ackPlanNow(rows) {
        const ack = state.ack;
        return PSL.ackPlan(rows, ack.checked, ack.reason, ack.expiry, Date.now());
    }

    function updateAckButtons(rows) {
        const acknowledge = document.getElementById('ack-submit');
        const revoke = document.getElementById('ack-revoke');
        const sentence = document.getElementById('ack-sentence');
        if (!acknowledge || !revoke || !sentence) return;
        const busy = state.ack.busy;
        const plan = ackPlanNow(rows);
        acknowledge.disabled = busy || plan.blocker !== null;
        revoke.disabled = busy || plan.revoke.length === 0;
        sentence.textContent = busy ? t('ack.writing') : plan.sentence;
        sentence.setAttribute('data-blocked', !busy && plan.blocked ? 'true' : 'false');
    }

    function ackForm(rows) {
        const ack = state.ack;
        const reason = el('input', {
            class: 'input', type: 'text', id: 'ack-reason', required: '', maxlength: '1024',
            value: ack.reason, autocomplete: 'off'
        });
        reason.addEventListener('input', function () {
            ack.reason = reason.value;
            updateAckButtons(rows);
        });
        // A day and no time: the ack holds through the last second of it, in UTC.
        const expiry = el('input', {
            class: 'input-date ack-expiry', type: 'date', id: 'ack-expiry', value: ack.expiry,
            min: new Date().toISOString().slice(0, 10)
        });
        const expiryLine = el('p', {
            class: 'field-note', id: 'ack-expiry-line', 'aria-live': 'polite',
            text: PSL.ackExpiryLine(ack.expiry, Date.now())
        });
        expiry.addEventListener('input', function () {
            ack.expiry = expiry.value;
            expiryLine.textContent = PSL.ackExpiryLine(expiry.value, Date.now());
            updateAckButtons(rows);
        });

        const acknowledge = el('button', {type: 'button', class: 'submit', id: 'ack-submit'}, [
            el('span', {text: t('ack.button.ack')})
        ]);
        acknowledge.addEventListener('click', function () {
            submitAck('ack', rows);
        });
        const revoke = el('button', {type: 'button', class: 'pill-button', id: 'ack-revoke'}, [
            el('span', {text: t('ack.button.revoke')})
        ]);
        revoke.addEventListener('click', function () {
            submitAck('revoke', rows);
        });
        queueMicrotask(function () {
            updateAckButtons(rows);
        });

        const actionable = rows.some(function (row) {
            return row.action !== 'none';
        });
        if (!actionable) {
            return el('div', {class: 'ack-stack'}, [
                el('div', {class: 'card params-panel'}, [
                    el('p', {
                        class: 'daemon-lead',
                        text: t(rows.length === 0 ? 'ack.none.no_source' : 'ack.none.no_writable')
                    }),
                    rows.length === 0 ? null : el('div', {class: 'field sources-wrap'}, [ackSourcesTable(rows)])
                ]),
                daemonAckBlock(rows)
            ]);
        }
        return el('form', {class: 'card params-panel'}, [
            field(t('ack.field.reason'), reason, t('ack.field.reason_gloss')),
            field(t('ack.field.expires'), expiry, t('ack.field.expires_gloss')),
            expiryLine,
            el('div', {class: 'field sources-wrap'}, [ackSourcesTable(rows)]),
            el('div', {class: 'submit-row'}, [
                acknowledge,
                revoke,
                el('p', {class: 'submit-sentence', id: 'ack-sentence'})
            ])
        ]);
    }

    /**
     * Where the Hub cannot write an ack, the daemon still can: its own command,
     * spelled for the daemon the link named, or the first one carrying the finding.
     */
    function daemonAckBlock(rows) {
        const route = PSL.readAckRoute(location.hash) || {};
        const daemons = rows.map(function (row) {
            return (state.sources || []).find(function (source) {
                return source.id === row.id && source.kind === 'daemon' && source.base_url;
            });
        }).filter(Boolean);
        const source = daemons.find(function (candidate) {
            return candidate.id === route.sourceId;
        }) || daemons[0];
        if (!source) return null;
        return terminalBlock({
            head: t('ack.daemon.head'),
            sub: t('ack.daemon.sub'),
            id: 'ack-command',
            spell: function (shellId) {
                return PSL.ackCommand(source, state.ack.signature, shellId);
            },
            copyLabel: t('ack.daemon.copy', {name: source.name}),
            notes: [
                t('ack.daemon.note'),
                engineNote()
            ]
        });
    }

    function ackSourcesTable(rows) {
        const head = el('tr', {}, ['', 'source', 'current', 'can_do'].map(function (name) {
            return el('th', {text: name ? t('ack.col.' + name) : '', scope: 'col'});
        }));
        return el('table', {class: 'table ack-table'}, [
            el('thead', {}, [head]),
            el('tbody', {}, rows.map(function (row) {
                const reasonId = 'ack-row-' + rows.indexOf(row);
                // The row's last cell says what this page can do there, and why not when the box is off.
                const box = el('input', {type: 'checkbox', 'aria-label': row.name, 'aria-describedby': reasonId});
                box.checked = PSL.ackChecked(row, state.ack.checked);
                box.disabled = row.action === 'none';
                box.addEventListener('change', function () {
                    // Kept with the action it was made on, see PSL.ackChecked.
                    state.ack.checked[row.id] = {action: row.action, checked: box.checked};
                    updateAckButtons(rows);
                });
                return el('tr', {}, [
                    el('td', {}, [box]),
                    el('td', {}, [
                        el('span', {class: 'table-strong', text: row.name}),
                        el('span', {text: ' '}),
                        el('span', {class: 'chip chip-declared', text: row.environment || t('incidents.unknown')})
                    ]),
                    ackCell(row.ack),
                    el('td', {
                        id: reasonId,
                        text: row.note || t(row.action === 'revoke' ? 'ack.row.revoke' : 'ack.row.ack')
                    })
                ]);
            }))
        ]);
    }

    /** `at` and `expires_at` are the daemon's own text, shown as it came. */
    function ackCell(ack) {
        if (!ack) return el('td', {}, [el('span', {class: 'table-muted', text: t('ack.cell.none')})]);
        return el('td', {}, [
            el('span', {class: 'table-strong', text: ack.by || '?'}),
            el('span', {
                text: ack.expires_at
                    ? t('ack.cell.until', {at: ack.at, until: ack.expires_at})
                    : t('ack.cell.no_expiry', {at: ack.at})
            }),
            ack.reason ? el('div', {class: 'table-muted', text: ack.reason}) : null
        ]);
    }

    function relayAck(row, action, body) {
        const path = '/api/sources/' + encodeURIComponent(row.id) + (action === 'revoke' ? '/acks/revoke' : '/acks');
        return fetch(path, {
            method: 'POST',
            headers: {'content-type': 'application/json'},
            body: JSON.stringify(body)
        }).then(signInOn401).then(function (response) {
            // A 204 has no body, and neither have the 413 and a full gate's 503.
            return response.json().catch(function () {
                return null;
            }).then(function (payload) {
                return {
                    name: row.name, action: action, status: response.status, detail: payload && payload.detail,
                    code: payload && payload.code
                };
            });
        }).catch(function () {
            return {name: row.name, action: action, status: 0};
        });
    }

    function submitAck(action, rows) {
        const ack = state.ack;
        const plan = ackPlanNow(rows);
        const targets = action === 'revoke' ? plan.revoke : plan.ack;
        if (ack.busy || targets.length === 0 || (action === 'ack' && plan.blocker)) return;
        const body = PSL.ackBody(action, ack.signature, ack.reason, plan.expiresAt);
        ack.busy = true;
        updateAckButtons(rows);

        const outcomes = [];
        // One after the other: the Hub relays two at a time and refuses a third.
        targets.reduce(function (chain, row) {
            return chain.then(function () {
                return relayAck(row, action, body);
            }).then(function (outcome) {
                outcomes.push(outcome);
            });
        }, Promise.resolve()).then(function () {
            ack.lines = PSL.ackSummary(outcomes);
            loadAck(true);
        });
    }

    // -------------------------------------------------------- screen: recap

    const RECAP_FIRST_PAGE = 10;
    const RECAP_PERIOD_CHOICES = ['day', 'week', 'month'];
    const RECAP_STATE_GLYPH = {
        confirmed: [['path', {d: 'M5 12.5l4.5 4.5L19 7.5'}]],
        contradicted: [['path', {d: 'M12 5v9M12 18.5v.01'}]],
        came_back: [['path', {d: 'M9 7L4 12l5 5'}], ['path', {d: 'M4 12h11a5 5 0 010 10h-2'}]],
        awaiting: [['circle', {cx: '12', cy: '12', r: '8.5'}], ['path', {d: 'M12 7.5V12l3 2'}]]
    };
    const RECAP_TODO_LABEL = ['todo', 'in_progress', 'done'];

    /** An action status as a row reads it, a status a newer Hub adds as it was sent. */
    function recapTodo(status) {
        return RECAP_TODO_LABEL.indexOf(status) >= 0 ? t('recap.status.' + status) : status;
    }

    function recapRoute() {
        return PSL.readRecapRoute(location.hash);
    }

    /** Writes the route the filters name; the hash change reads the recap again. */
    function goRecap(change) {
        location.hash = PSL.recapHash(Object.assign({}, recapRoute(), change));
    }

    /**
     * Reads the recap the route names. A write on a row reads it again with `keep`, so the page
     * stays where it was while the new figures arrive. Only the latest read lands.
     */
    function loadRecap(keep) {
        const sequence = ++state.recapSeq;
        if (!keep) {
            state.recap = null;
            state.recapError = null;
            state.recapRowError = {};
            state.recapNaming = null;
            state.recapNameDraft = null;
            state.recapShowAll = false;
            state.recapReports = null;
            state.recapGenerateError = null;
            render();
        }
        Promise.all([getJson(PSL.recapPath(recapRoute())), readRecapReports()]).then(function (results) {
            if (sequence !== state.recapSeq) return;
            state.recap = results[0];
            state.recapReports = results[1];
            state.recapError = null;
        }).catch(function (error) {
            if (sequence !== state.recapSeq) return;
            state.recap = null;
            state.recapError = error.status || 0;
        }).finally(function () {
            if (sequence === state.recapSeq && currentScreen() === 'recap') render();
        });
    }

    /** A 403 is a caller with no right to the reports: null hides the section. */
    function readRecapReports() {
        return getJson('/api/recap/reports').catch(function (error) {
            return error.status === 403 ? null : 'error';
        });
    }

    function renderRecapScreen() {
        const route = recapRoute();
        const section = el('section', {}, [
            ruledOverline(t('recap.overline')),
            el('h1', {class: 'page-title', text: t('recap.title')}),
            el('p', {class: 'page-sub', text: t('recap.sub')}),
            recapFilterLine(route)
        ]);
        if (state.recapError !== null) {
            section.appendChild(recapErrorBanner(state.recapError, route.view));
            return section;
        }
        if (state.recap === null) {
            section.appendChild(el('div', {class: 'card skeleton', style: 'height:220px'}));
            return section;
        }
        const snapshot = state.recap;
        const labels = PSL.RECAP_LABELS[snapshot.view === 'pilotage' ? 'pilotage' : 'tech'];
        [recapPeriodBlock(snapshot), recapTiles(snapshot, labels), recapPriorities(snapshot, route),
            recapMoved(snapshot, labels), recapReportsPanel(route), recapCardPanel(route)].forEach(function (part) {
            if (part) section.appendChild(part);
        });
        return section;
    }

    function recapErrorBanner(status, view) {
        const sentence = PSL.recapErrorText(status, view);
        return el('div', {
            class: 'banner',
            'data-tone': 'crit',
            role: 'alert'
        }, [critGlyph(16), el('div', {text: sentence})]);
    }

    function recapFilterLine(route) {
        const period = el('select', {class: 'refresh-select', 'aria-label': t('recap.filter.period')},
            RECAP_PERIOD_CHOICES.map(function (kind) {
                const option = el('option', {value: kind, text: t('recap.filter.period.' + kind)});
                if (kind === route.period) option.selected = true;
                return option;
            }));
        period.addEventListener('change', function () {
            goRecap({period: period.value});
        });
        const day = el('input', {
            class: 'recap-input', type: 'date', value: route.at || '', 'aria-label': t('recap.filter.day'),
            max: new Date().toISOString().slice(0, 10)
        });
        day.addEventListener('change', function () {
            goRecap({at: day.value || null});
        });
        const environments = Array.from(new Set((state.sources || []).map(function (source) {
            return source.environment;
        }).filter(Boolean))).sort();
        return el('div', {class: 'refresh recap-filter'}, [
            el('span', {class: 'refresh-label', text: t('recap.filter.period')}),
            period,
            day,
            filterSelect('environment', route.environment || '', environments.map(function (name) {
                return [name, name];
            }), function (value) {
                goRecap({environment: value || null});
            }),
            recapTextFilter(t('recap.filter.service'), route.service, 'service'),
            recapTextFilter(t('recap.filter.namespace'), route.grouping, 'grouping'),
            recapViewSwitch(route)
        ]);
    }

    /** Applied on change, so a name is not searched for a letter at a time. */
    function recapTextFilter(label, value, key) {
        const input = el('input', {
            class: 'recap-input', type: 'text', placeholder: label, value: value || '', 'aria-label': label,
            spellcheck: 'false'
        });
        input.addEventListener('change', function () {
            goRecap({[key]: input.value.trim() || null});
        });
        return input;
    }

    function recapViewSwitch(route) {
        const group = el('div', {class: 'segmented', role: 'radiogroup', 'aria-label': t('recap.view.label')});
        ['tech', 'pilotage'].forEach(function (view) {
            const button = el('button', {
                type: 'button', role: 'radio', 'aria-checked': route.view === view ? 'true' : 'false',
                text: t('recap.view.' + view)
            });
            button.addEventListener('click', function () {
                goRecap({view: view});
            });
            group.appendChild(button);
        });
        return radioKeys(group, 'recap-view');
    }

    /** A heading with its "?" beside it, never inside it, centred on the heading's line. */
    function recapHeading(id, text, hint, help, topic) {
        return el('div', {class: 'title-row'}, [
            el('h2', {
                class: 'recap-h2',
                id: id,
                tabindex: '-1'
            }, [document.createTextNode(text), hint ? el('span', {text: hint}) : null]),
            helpDot(help, topic)
        ]);
    }

    function recapHelp(snapshot, key) {
        return PSL.RECAP_HELP[snapshot.view === 'pilotage' ? 'pilotage' : 'tech'][key];
    }

    function recapNotes(text) {
        return text ? el('p', {class: 'recap-notes', text: text}) : null;
    }

    function recapPeriodBlock(snapshot) {
        const period = snapshot.period;
        const previous = snapshot.previous;
        const view = snapshot.view;
        const previousNotes = PSL.notesText(previous.notes, previous.sampling_rate, view);
        const reasons = [PSL.incompleteText(period.incomplete, view)];
        const previousReasons = PSL.incompleteText(previous.incomplete, view);
        if (previousReasons) {
            reasons.push(t('recap.incomplete.previous', {
                reasons: previous.incomplete.map(function (reason) {
                    return PSL.gapLabel(reason, view);
                }).join(', ')
            }));
        }
        const block = el('div', {class: 'recap-period-block'}, [
            el('div', {class: 'recap-period'}, [
                el('span', {class: 'recap-period-main', text: PSL.periodText(period)}),
                el('span', {class: 'recap-period-tz', text: 'UTC'}),
                el('span', {class: 'recap-period-prev', text: recapAgainst('recap.period.compared.', previous)}),
                helpDot(recapHelp(snapshot, 'period'), t('recap.topic.period'))
            ]),
            recapNotes(PSL.notesText(period.notes, period.sampling_rate, view)),
            recapNotes(previousNotes ? t('recap.notes.previous', {notes: previousNotes}) : '')
        ]);
        if (reasons.some(Boolean)) {
            block.appendChild(el('div', {class: 'banner', 'data-tone': 'warn', role: 'note'}, [
                warningGlyph(16),
                el('div', {text: t('recap.banner', {reasons: reasons.filter(Boolean).join(t('recap.banner.join'))})})
            ]));
        }
        return block;
    }

    /** "compared with" or "since" the previous period, worded for a day or for a range of days. */
    function recapAgainst(prefix, previous) {
        return t(prefix + (previous.kind === 'day' ? 'day' : 'range'), {period: PSL.periodText(previous)});
    }

    function recapTile(label, chip, value, subs, help, topic) {
        return el('div', {class: 'disc-figure'}, [
            el('span', {class: 'disc-figure-label'}, [document.createTextNode(label), chip, helpDot(help, topic)]),
            value
        ].concat(subs.filter(Boolean).map(function (sub) {
            return el('span', {class: 'disc-figure-sub', text: sub});
        })));
    }

    function recapProgressChip(snapshot) {
        return snapshot.period.in_progress
            ? el('span', {class: 'chip', 'data-tone': 'progress', text: t('recap.chip.in_progress')})
            : null;
    }

    /** "previous week 168,900", with the days it was cut to when this period is not over. */
    function recapPreviousText(snapshot, value) {
        if (value === null || value === undefined) return '';
        const days = snapshot.previous.truncated_to_days;
        return t('recap.previous.' + snapshot.previous.kind, {
            cut: days ? t('recap.previous.cut.' + PSL.plural(days), {count: days}) : '',
            value: value
        });
    }

    /** A count with thousands separators, or null when the Hub has none. */
    function recapCount(value) {
        return value === null || value === undefined ? null : PSL.num(value);
    }

    function recapCo2Tile(snapshot) {
        const tiles = snapshot.tiles;
        const without = tiles.co2_sources_without_coefficient;
        const co2 = PSL.co2Parts(tiles.co2_grams, without, tiles.co2_missing_reason, snapshot.view);
        const known = tiles.co2_grams !== null && tiles.co2_grams !== undefined;
        const previous = tiles.previous_co2_grams === null || tiles.previous_co2_grams === undefined
            ? null : PSL.co2Parts(tiles.previous_co2_grams, 0, null).figure;
        // The full sentence stays on the figure for a reader the small print does not reach.
        const value = el('span', {
            class: 'disc-figure-value',
            title: PSL.co2Text(tiles.co2_grams, without, tiles.co2_missing_reason, snapshot.view)
        }, [
            co2.lead ? el('small', {text: co2.lead}) : null,
            document.createTextNode(co2.figure),
            el('span', {class: 'disc-figure-unit', text: co2.unit})
        ]);
        return recapTile(t('recap.tile.co2'), recapProgressChip(snapshot), value, [
            known ? t('recap.tile.magnitude', {tail: PSL.co2Tail(without, snapshot.view)}) : '',
            previous ? recapPreviousText(snapshot, previous) : ''
        ], recapHelp(snapshot, 'co2'), t('recap.topic.co2'));
    }

    function recapTiles(snapshot, labels) {
        const tiles = snapshot.tiles;
        const actions = tiles.actions;
        const management = snapshot.view === 'pilotage';
        return el('div', {class: 'disc-figures recap-tiles'}, [
            recapTile(labels.io, recapProgressChip(snapshot),
                el('span', {class: 'disc-figure-value', text: recapCount(tiles.avoidable_io_ops) || '\u2014'}),
                [recapPreviousText(snapshot, recapCount(tiles.previous_avoidable_io_ops))],
                recapHelp(snapshot, 'io'), labels.io),
            recapCo2Tile(snapshot),
            recapTile(labels.findings, null,
                el('span', {class: 'disc-figure-value', text: String(tiles.active_findings)}), [
                    recapPreviousText(snapshot, tiles.previous_active_findings),
                    t('recap.tile.acked.' + (management ? 'pilotage.' : 'tech.') + PSL.plural(tiles.acked_findings),
                        {count: tiles.acked_findings})
                ], recapHelp(snapshot, 'findings'), labels.findings),
            recapTile(t('recap.tile.actions'), null, el('span', {class: 'disc-figure-value'}, [
                document.createTextNode(String(actions.done)),
                el('span', {class: 'disc-figure-unit', text: t('recap.tile.done.' + PSL.plural(actions.done))})
            ]), [
                t('recap.tile.actions.checked', {
                    confirmed: actions.confirmed, contradicted: actions.contradicted, came_back: actions.came_back
                }),
                t('recap.tile.actions.open', {in_progress: actions.in_progress, todo: actions.todo})
            ], recapHelp(snapshot, 'actions'), t('recap.topic.actions'))
        ]);
    }

    // ---- the priorities

    function recapPriorities(snapshot, route) {
        const management = snapshot.view === 'pilotage';
        const all = snapshot.priorities;
        const shown = management || state.recapShowAll ? all : all.slice(0, RECAP_FIRST_PAGE);
        const hint = t('recap.prio.hint', {io: recapIoLower(snapshot)})
            + (management ? '' : t('recap.prio.hint.co2'))
            + (snapshot.period.in_progress ? t('recap.prio.hint.closed') : '');
        const max = all.length && all[0].avoidable_io_ops ? all[0].avoidable_io_ops : 0;
        const list = el('ol', {class: 'recap-list'});
        shown.forEach(function (priority, index) {
            if (PSL.dividesBefore(all, index)) {
                list.appendChild(el('li', {
                    class: 'recap-divider',
                    text: t('recap.prio.divider.' + (management ? 'pilotage' : 'tech'))
                }));
            }
            list.appendChild(recapRow(priority, max, snapshot, route, PSL.namesService(shown, index)));
        });
        const section = el('section', {class: 'recap-section', 'aria-labelledby': 'recap-h-prio'}, [
            ruledOverline(t('recap.prio.overline')),
            recapHeading('recap-h-prio', t('recap.prio.title'), hint, recapHelp(snapshot, 'prio'), t('recap.topic.ranking'))
        ]);
        if (all.length === 0) {
            section.appendChild(el('div', {class: 'empty-state'}, [
                el('p', {class: 'empty-title', text: t('recap.prio.empty')})
            ]));
            return section;
        }
        section.appendChild(list);
        recapPriorityNotes(snapshot, all, section);
        return section;
    }

    function recapPriorityNotes(snapshot, all, section) {
        if (snapshot.view !== 'pilotage' && all.length > RECAP_FIRST_PAGE && !state.recapShowAll) {
            const more = el('button', {
                class: 'pill-button recap-showall',
                type: 'button',
                text: t('recap.prio.show_all', {count: all.length})
            });
            more.addEventListener('click', function () {
                state.recapShowAll = true;
                renderFocus('#recap-h-prio');
            });
            section.appendChild(more);
        }
        if (snapshot.view === 'pilotage' && snapshot.more_priorities > 0) {
            section.appendChild(el('p', {
                class: 'recap-more',
                text: t('recap.prio.more.' + PSL.plural(snapshot.more_priorities), {count: snapshot.more_priorities})
            }));
        }
        if (snapshot.truncated) {
            section.appendChild(el('p', {class: 'sources-note', text: PSL.recapTruncatedText(snapshot.view)}));
        }
    }

    /** The volume's label inside a sentence: "avoidable I/O", never a capital mid-line. */
    function recapIoLower(snapshot) {
        return t('recap.label.' + (snapshot.view === 'pilotage' ? 'pilotage' : 'tech') + '.io_lower');
    }

    function recapRow(priority, max, snapshot, route, namesService) {
        const management = snapshot.view === 'pilotage';
        const kids = [
            el('span', {class: 'recap-rank', text: String(priority.rank)}),
            management ? recapPlainWhat(priority) : recapTechWhat(priority),
            recapFigure(priority, max, recapIoLower(snapshot), management)
        ];
        if (!management) kids.push(recapDetail(priority));
        kids.push(recapFoot(priority, management, route, namesService));
        const naming = management ? null : recapNamingTarget(priority);
        if (naming) kids.push(recapNameEditor(naming));
        const failure = management ? null : state.recapRowError[priority.signature];
        if (failure) kids.push(el('p', {class: 'recap-alert', role: 'alert', text: failure}));
        return el('li', {class: 'run-card recap-row'}, kids);
    }

    function recapTechWhat(priority) {
        return el('div', {class: 'recap-what'}, [
            priority.label ? el('p', {class: 'recap-label', text: priority.label}) : null,
            el('div', {class: 'recap-head'}, [
                el('span', {
                    class: 'chip', 'data-type': priority.finding_type,
                    text: PSL.FINDING_TYPE_LABEL[priority.finding_type] || priority.finding_type
                }),
                recapServiceHead(priority),
                el('span', {class: 'recap-endpoint', text: priority.endpoint})
            ]),
            priority.suggestion ? engineText(proseInto(el('p', {class: 'recap-suggest'}), priority.suggestion)) : null
        ]);
    }

    /** The label when someone named the endpoint, else the sentence and the one technical value left. */
    function recapPlainWhat(item) {
        const service = PSL.serviceParts(item.service, item.service_label).name;
        return el('div', {class: 'recap-what'}, [
            item.label ? el('p', {class: 'recap-label', text: item.label}) : null,
            el('p', {class: 'recap-plain', text: PSL.recapPlain(item)}),
            el('div', {class: 'recap-where'}, item.label || !item.endpoint
                ? [document.createTextNode(t('recap.where', {service: service}))]
                : [document.createTextNode(t('recap.where', {service: service}) + ' · '),
                    el('code', {class: 'code-inline', text: item.endpoint})])
        ]);
    }

    /** The service by the name a team gave it, the technical one beside it once named. */
    function recapServiceHead(priority) {
        const parts = PSL.serviceParts(priority.service, priority.service_label);
        return el('span', {class: 'recap-service'}, [
            el('span', {class: 'table-strong', text: parts.name}),
            parts.code ? el('code', {class: 'code-inline', text: parts.code}) : null
        ]);
    }

    function recapTrend(item) {
        const text = PSL.trendText(item.trend, item.change);
        return text
            ? el('span', {
                class: 'recap-trend',
                'data-trend': item.trend,
                title: t('recap.trend.title'),
                text: text
            })
            : null;
    }

    function recapFigure(priority, max, lower, management) {
        if (!PSL.hasAvoidableVolume(priority)) {
            return el('div', {class: 'recap-figure'}, [
                el('span', {class: 'recap-figure-n', text: '—'}),
                el('span', {class: 'recap-figure-u', text: t(management ? 'recap.figure.none' : 'recap.co2.no_avoidable_io')}),
                recapTrend(priority)
            ]);
        }
        const share = max > 0 ? Math.max(2, Math.round(priority.avoidable_io_ops / max * 100)) : 2;
        return el('div', {class: 'recap-figure'}, [
            el('span', {class: 'recap-figure-n', text: recapCount(priority.avoidable_io_ops)}),
            el('span', {class: 'recap-figure-u', text: lower}),
            el('span', {class: 'recap-bar', 'aria-hidden': 'true'}, [el('i', {style: 'width:' + share + '%'})]),
            recapTrend(priority)
        ]);
    }

    // ---- the folded technical detail (Engineering)

    function recapDetail(priority) {
        const key = 'recap:' + priority.signature;
        const open = state.panelOpen[key] === undefined ? priority.rank === 1 : state.panelOpen[key] === true;
        const details = el('details', {class: 'recap-detail'}, [
            el('summary', {}, [svg([['path', {d: 'M9 6l6 6-6 6'}]], 12), document.createTextNode(t('recap.fix.summary'))]),
            el('div', {class: 'recap-detail-body'}, [
                priority.template ? el('pre', {class: 'recap-code', text: priority.template}) : null,
                recapFix(priority.suggested_fix),
                recapCodeLocation(priority.code_location),
                recapFacts(priority),
                recapEvents(priority.action)
            ])
        ]);
        if (open) details.open = true;
        details.addEventListener('toggle', function () {
            state.panelOpen[key] = details.open;
        });
        return details;
    }

    function recapFix(fix) {
        if (!fix) return null;
        const url = PSL.httpUrl(fix.reference_url);
        let reference = null;
        if (fix.reference_url) {
            reference = el('div', {class: 'recap-fix-ref'}, [url
                ? el('a', {href: url, rel: 'noopener noreferrer', target: '_blank', text: t('recap.fix.reference')})
                : el('span', {text: t('recap.fix.reference_at', {url: fix.reference_url})})]);
        }
        return el('div', {class: 'recap-fix'}, [
            el('div', {class: 'recap-fix-head'}, [
                document.createTextNode(t('recap.fix.suggested')),
                fix.framework ? el('span', {class: 'chip', text: fix.framework}) : null
            ]),
            engineText(proseInto(el('div'), fix.recommendation)),
            reference
        ]);
    }

    /** The function on one line and `path:line` on the next, as a stack trace reads. */
    function recapCodeLocation(location) {
        if (!location) return null;
        const at = [location.filepath, location.lineno].filter(function (part) {
            return part !== null && part !== undefined && part !== '';
        }).join(':');
        const lines = [location.function, at].filter(Boolean).join('\n');
        if (!lines) return null;
        return el('div', {}, [el('div', {
            class: 'recap-fix-head',
            text: t('recap.fix.where')
        }), el('pre', {class: 'recap-code', text: lines})]);
    }

    function recapFacts(priority) {
        const rows = [
            [t('recap.fact.detections'), priority.detections === null || priority.detections === undefined
                ? '—' : PSL.num(priority.detections)],
            [t('recap.fact.confidence'), priority.confidence || '—']
        ];
        const list = el('dl', {class: 'recap-kv'});
        rows.forEach(function (row) {
            list.appendChild(el('dt', {text: row[0]}));
            list.appendChild(el('dd', {text: row[1]}));
        });
        list.appendChild(el('dt', {text: t('recap.fact.signature')}));
        list.appendChild(el('dd', {}, [el('code', {class: 'code-inline', text: priority.signature})]));
        return list;
    }

    /** Every change of the action, newest last, in the reader's own time. */
    function recapEvents(action) {
        if (!action || !action.events || !action.events.length) return null;
        return el('div', {}, [
            el('div', {class: 'recap-fix-head', text: t('recap.events.title')}),
            el('ul', {class: 'recap-events'}, action.events.map(function (event) {
                const owner = PSL.ownerLabel(event.owner, event.owner_display);
                return el('li', {}, [
                    el('time', {
                        datetime: PSL.isoUtc(event.at_ms),
                        title: PSL.isoUtc(event.at_ms),
                        text: PSL.localTime(event.at_ms)
                    }),
                    document.createTextNode((event.from ? recapTodo(event.from) + ' → ' : '')
                        + recapTodo(event.to) + (owner.text ? ', ' + owner.text : '')
                        + t('recap.event.by', {who: event.by}))
                ]);
            }))
        ]);
    }

    // ---- actions, names and the foot of a row

    function recapStateChip(kind, text) {
        return el('span', {class: 'recap-state', 'data-state': kind}, [
            RECAP_STATE_GLYPH[kind] ? svg(RECAP_STATE_GLYPH[kind], 12) : null,
            document.createTextNode(text)
        ]);
    }

    function recapWhen(action) {
        const done = action.status === 'done' && action.done_at_ms;
        const at = done ? action.done_at_ms : action.updated_at_ms;
        return el('time', {
            class: 'recap-when', datetime: PSL.isoUtc(at), title: PSL.isoUtc(at),
            text: t(done ? 'recap.when.done' : 'recap.when.updated', {time: PSL.localTime(at)})
        });
    }

    function recapOwner(action) {
        const owner = PSL.ownerLabel(action.owner, action.owner_display);
        return owner.text ? el('span', {class: 'recap-owner', title: owner.title, text: owner.text}) : null;
    }

    /** Management reads an action and cannot change it. */
    function recapActionReadOnly(action) {
        if (!action) return [recapStateChip('none', t('recap.state.not_taken'))];
        if (action.status !== 'done') {
            return [recapStateChip(action.status === 'todo' ? 'none' : 'in_progress',
                t(action.status === 'todo' ? 'recap.state.todo' : 'recap.state.in_progress')), recapOwner(action),
                recapWhen(action)];
        }
        const confirmation = PSL.confirmationLabel(action.confirmation, 'pilotage');
        return [recapStateChip(confirmation ? action.confirmation : 'none',
            confirmation ? t('recap.state.done_with', {label: confirmation.toLowerCase()}) : t('recap.state.done')),
            recapOwner(action), recapWhen(action)];
    }

    function recapActionControls(priority, route) {
        const action = priority.action;
        const busy = state.recapBusy[priority.signature] === true;
        const ack = el('a', {
            class: 'ack-link', href: PSL.ackRouteHash(priority.signature, null, route.environment),
            text: t('recap.action.ack')
        });
        ack.addEventListener('click', function () {
            state.ackReturn = location.hash;
        });
        if (!action) {
            const take = el('button', {
                class: 'pill-button pill-sm',
                type: 'button',
                text: t('recap.action.take'),
                disabled: busy ? 'disabled' : null
            });
            take.addEventListener('click', function () {
                recapWrite(priority.signature, 'POST', '/api/actions', {signature: priority.signature, take: true});
            });
            return [take, ack];
        }
        const select = el('select', {
                class: 'refresh-select',
                'aria-label': t('recap.action.status'),
                disabled: busy ? 'disabled' : null
            },
            RECAP_TODO_LABEL.map(function (status) {
                const option = el('option', {value: status, text: recapTodo(status)});
                if (status === action.status) option.selected = true;
                return option;
            }));
        select.addEventListener('change', function () {
            recapWrite(priority.signature, 'PATCH', '/api/actions', {
                signature: priority.signature,
                status: select.value
            });
        });
        const confirmation = PSL.confirmationLabel(action.confirmation);
        return [recapOwner(action), select,
            confirmation ? recapStateChip(action.confirmation, confirmation) : null, recapWhen(action), ack];
    }

    /** `namesService` puts the service button on the first card of a service only, beside "Name this". */
    function recapFoot(priority, management, route, namesService) {
        const kids = [];
        if (!management) {
            kids.push(el('span', {class: 'facts'}, [
                el('span', {}, [
                    el('b', {
                        text: priority.detections === null || priority.detections === undefined
                            ? '—' : PSL.num(priority.detections)
                    }),
                    document.createTextNode(t('recap.foot.detections'))
                ]),
                recapNameButton(recapNameTarget(priority, 'endpoint')),
                namesService ? recapNameButton(recapNameTarget(priority, 'service')) : null
            ]));
        }
        kids.push(el('div', {class: 'recap-action'}, management
            ? recapActionReadOnly(priority.action) : recapActionControls(priority, route)));
        return el('div', {class: 'recap-foot'}, kids);
    }

    /**
     * Sends a write on a row and reads the recap again, whatever it answered. A refusal keeps the
     * Hub's own sentence in the row: it names why (no identity, no permission).
     */
    function recapSend(method, path, body) {
        return fetch(path, {
            method: method,
            headers: {'content-type': 'application/json', accept: 'application/json'},
            body: JSON.stringify(body)
        }).then(signInOn401).then(function (response) {
            if (response.ok) return null;
            return response.json().catch(function () {
                return null;
            }).then(function (payload) {
                return PSL.problemText(payload, response.status);
            });
        }).catch(function () {
            return t('recap.send.failed');
        });
    }

    function recapWrite(signature, method, path, body) {
        if (state.recapBusy[signature]) return;
        state.recapBusy[signature] = true;
        render();
        recapSend(method, path, body).then(function (failure) {
            delete state.recapBusy[signature];
            if (failure) state.recapRowError[signature] = failure;
            else delete state.recapRowError[signature];
            loadRecap(true);
        });
    }

    /**
     * What the inline editor names on a row: its endpoint, or its whole service. One key per target, so
     * the two editors of a row never open together; a refusal lands on the row either way.
     */
    function recapNameTarget(priority, kind) {
        if (kind === 'service') {
            return {
                key: 'service ' + priority.signature,
                row: priority.signature,
                label: priority.service_label,
                subject: t('recap.name.service.subject', {service: priority.service}),
                name: t('recap.name.service'),
                rename: t('recap.name.service.rename'),
                placeholder: t('recap.name.service.placeholder'),
                path: '/api/labels/services',
                body: {service: priority.service}
            };
        }
        return {
            key: priority.signature,
            row: priority.signature,
            label: priority.label,
            subject: priority.service + ' ' + priority.endpoint,
            name: t('recap.name.endpoint'),
            rename: t('recap.name.endpoint.rename'),
            placeholder: t('recap.name.endpoint.placeholder'),
            path: '/api/labels',
            body: {service: priority.service, endpoint: priority.endpoint}
        };
    }

    /** The target whose editor is open on this row, if any. */
    function recapNamingTarget(priority) {
        return ['endpoint', 'service'].map(function (kind) {
            return recapNameTarget(priority, kind);
        }).find(function (target) {
            return target.key === state.recapNaming;
        }) || null;
    }

    function recapNameButton(target) {
        const button = el('button', {
            class: 'recap-link', type: 'button', text: target.label ? target.rename : target.name,
            'aria-label': t(target.label ? 'recap.name.aria.rename' : 'recap.name.aria', {subject: target.subject})
        });
        button.addEventListener('click', function () {
            state.recapNaming = state.recapNaming === target.key ? null : target.key;
            state.recapNameDraft = null;
            renderFocus('.recap-name-input');
        });
        return button;
    }

    /** A business name, in a line of its own under the row. Enter saves, an empty name removes it. */
    function recapNameEditor(target) {
        const input = el('input', {
            class: 'recap-input recap-name-input',
            type: 'text',
            maxlength: '80',
            value: state.recapNameDraft !== null ? state.recapNameDraft : target.label || '',
            placeholder: target.placeholder,
            'aria-label': t('recap.name.input', {subject: target.subject}),
            spellcheck: 'false'
        });
        const save = el('button', {class: 'pill-button pill-sm', type: 'submit', text: t('recap.name.save')});
        const cancel = el('button', {class: 'recap-link', type: 'button', text: t('recap.name.cancel')});
        cancel.addEventListener('click', function () {
            state.recapNaming = null;
            state.recapNameDraft = null;
            render();
        });
        const form = el('form', {class: 'recap-name'}, [input, save, cancel]);
        form.addEventListener('submit', function (event) {
            event.preventDefault();
            recapNameSave(target, input.value.trim() || null);
        });
        return form;
    }

    function recapNameSave(target, label) {
        recapSend('PUT', target.path, Object.assign({}, target.body, {label: label}))
            .then(function (failure) {
                if (failure) {
                    state.recapRowError[target.row] = failure;
                    state.recapNameDraft = label === null ? '' : label;
                    render();
                    return;
                }
                delete state.recapRowError[target.row];
                state.recapNaming = null;
                state.recapNameDraft = null;
                loadRecap(true);
            });
    }

    // ---- what moved

    /** The sentence in a line, led by the label when the endpoint has one. */
    function recapMoveWho(item) {
        const service = PSL.serviceParts(item.service, item.service_label).name;
        const where = item.label || !item.endpoint
            ? [document.createTextNode(t('recap.where.move', {service: service}))]
            : [document.createTextNode(t('recap.where.move.at', {service: service})), el('code', {
                class: 'code-inline',
                text: item.endpoint
            }),
                document.createTextNode('.')];
        return el('span', {class: 'who'}, [
            el('span', {class: 'plain'}, [
                item.label ? el('b', {class: 'recap-label', text: item.label + '. '}) : null,
                document.createTextNode(PSL.recapPlain(item))
            ]),
            el('span', {class: 'recap-where'}, where)
        ]);
    }

    function recapMoveItem(item, management) {
        const who = management
            ? recapMoveWho(item)
            : el('span', {class: 'who'}, [
                el('span', {
                    class: 'chip', 'data-type': item.finding_type,
                    text: PSL.FINDING_TYPE_LABEL[item.finding_type] || item.finding_type
                }),
                el('span', {class: 'table-strong', text: item.service}),
                el('span', {class: 'recap-endpoint', text: item.endpoint})
            ]);
        return el('li', {}, [who, recapTrend(item)]);
    }

    function recapMoved(snapshot, labels) {
        const management = snapshot.view === 'pilotage';
        const moves = snapshot.movements;
        const confirmed = moves.gone_with_confirmed_action;
        const groups = [
            [t('recap.moves.new'), moves.new, '', ''],
            [t('recap.moves.gone'), moves.gone,
                confirmed > 0 ? t('recap.moves.confirmed.' + PSL.plural(confirmed), {count: confirmed}) : '', ''],
            [t('recap.moves.up'), moves.up, '', 'crit'],
            [t('recap.moves.down'), moves.down, '', ''],
            [t('recap.moves.came_back'), moves.came_back, '', 'warn'],
            [t('recap.moves.done_still'), moves.done_still_detected, '', 'crit']
        ].filter(function (group) {
            return group[1].length > 0;
        });
        const section = el('section', {class: 'recap-section', 'aria-labelledby': 'recap-h-moved'}, [
            ruledOverline('// ' + labels.moved.toLowerCase()),
            recapHeading('recap-h-moved', labels.moved, recapAgainst('recap.moves.since.', snapshot.previous),
                recapHelp(snapshot, 'moved'), labels.moved)
        ]);
        const note = recapMovedNote(snapshot);
        if (note) section.appendChild(el('p', {class: 'recap-more', text: note}));
        if (groups.length) {
            section.appendChild(el('div', {class: 'recap-moves'}, groups.map(function (group) {
                return el('div', {class: 'recap-move', 'data-tone': group[3] || null}, [
                    el('h3', {}, [
                        document.createTextNode(group[0]),
                        el('span', {class: 'n', text: String(group[1].length)}),
                        group[2] ? el('span', {class: 'sub', text: group[2]}) : null
                    ]),
                    el('ul', {}, group[1].map(function (item) {
                        return recapMoveItem(item, management);
                    }))
                ]);
            })));
        }
        if (snapshot.sources_without_volume.length) {
            section.appendChild(el('p', {
                class: 'sources-note', text: PSL.recapSourcesNote(snapshot.sources_without_volume, snapshot.view)
            }));
        }
        if (snapshot.late_sources && snapshot.late_sources.length) {
            section.appendChild(el('p', {class: 'sources-note', text: PSL.recapLateNote(snapshot.late_sources)}));
        }
        return section;
    }

    /**
     * The first day of a period has no closed day to compare, which is not the same as nothing
     * having moved: the server's `nothing_moved` is true there only because every list is empty.
     */
    function recapMovedNote(snapshot) {
        if (snapshot.previous.truncated_to_days === 0) return t('recap.moves.none_closed');
        return snapshot.nothing_moved ? t('recap.moves.nothing') : '';
    }

    // ---- the frozen reports

    const RECAP_REPORT_VIEWS = ['pilotage', 'tech'];
    const RECAP_DELIVERY_STATUS = {delivered: 'succeeded', failed: 'failed'};

    /** The list under What moved; null (no right to read it) leaves the section out. */
    function recapReportsPanel(route) {
        const reports = state.recapReports;
        if (reports === null) return null;
        const section = el('section', {class: 'recap-section', 'aria-labelledby': 'recap-h-reports'}, [
            ruledOverline(t('recap.reports.overline')),
            recapHeading('recap-h-reports', PSL.RECAP_REPORTS_COPY.title, PSL.RECAP_REPORTS_COPY.hint,
                PSL.RECAP_REPORTS_COPY.help, t('recap.topic.reports')),
            recapGenerateBar(route)
        ]);
        if (state.recapGenerateError) {
            section.appendChild(el('p', {
                class: 'recap-alert recap-alert-wide',
                role: 'alert',
                text: state.recapGenerateError
            }));
        }
        if (reports === 'error') {
            section.appendChild(el('p', {class: 'sources-note', text: PSL.RECAP_REPORTS_COPY.failed}));
        } else if (reports.length === 0) {
            section.appendChild(el('p', {class: 'recap-more', text: PSL.RECAP_REPORTS_COPY.none}));
        } else {
            section.appendChild(el('ul', {class: 'recap-list'}, reports.map(recapReportRow)));
        }
        return section;
    }

    function recapGenerateBar(route) {
        const button = el('button', {
            class: 'pill-button pill-sm recap-generate-button',
            type: 'button',
            disabled: state.recapGenerating ? 'disabled' : null,
            text: state.recapGenerating ? PSL.RECAP_REPORTS_COPY.generating : PSL.RECAP_REPORTS_COPY.generate
        });
        button.addEventListener('click', function () {
            recapGenerate(route);
        });
        return el('div', {class: 'recap-generate'}, [
            el('span', {class: 'recap-more', text: PSL.recapFreezeText(route)}),
            button
        ]);
    }

    function recapGenerate(route) {
        if (state.recapGenerating) return;
        const sequence = state.recapSeq;
        state.recapGenerating = true;
        state.recapGenerateError = null;
        render();
        // An on-demand report takes the language its reader is shown.
        const body = Object.assign(PSL.recapOnDemandBody(route), {lang: PSL.lang()});
        recapSend('POST', '/api/recap/reports', body).then(function (failure) {
            state.recapGenerating = false;
            // Management reads no permission name nor filter key: the Hub's own detail is for engineers.
            state.recapGenerateError = failure && route.view === 'pilotage' ? PSL.RECAP_REPORTS_COPY.refused : failure;
            // The button was replaced while it was busy: put the focus back on it.
            if (failure) return renderFocus('.recap-generate-button');
            return readRecapReports().then(function (reports) {
                if (sequence === state.recapSeq) state.recapReports = reports;
                if (currentScreen() === 'recap') renderFocus('.recap-generate-button');
            });
        });
    }

    function recapReportRow(report) {
        const status = PSL.recapReportStatus(report);
        const links = RECAP_REPORT_VIEWS.filter(function (view) {
            return report.views.indexOf(view) >= 0;
        }).map(function (view) {
            const name = t('recap.view.' + view);
            return el('a', {
                class: 'recap-link', href: '/recaps/' + report.id + '/' + view + '.html', download: '',
                text: name, 'aria-label': t('recap.report.view_aria', {view: name, report: PSL.recapReportLabel(report)})
            });
        });
        return el('li', {class: 'run-card recap-report'}, [
            el('p', {class: 'recap-label', text: PSL.recapReportLabel(report)}),
            el('div', {class: 'recap-report-links'}, links),
            el('div', {class: 'recap-report-foot'}, [
                el('time', {
                    class: 'recap-when',
                    datetime: PSL.isoUtc(report.generated_at_ms),
                    title: PSL.isoUtc(report.generated_at_ms),
                    text: t('recap.report.generated', {time: PSL.localTime(report.generated_at_ms)})
                }),
                status ? el('span', {class: 'recap-report-status', text: status}) : null
            ].concat(report.deliveries.map(function (delivery) {
                return el('span', {
                    class: 'status-pill', 'data-status': RECAP_DELIVERY_STATUS[delivery.state] || 'queued',
                    text: PSL.recapDeliveryText(delivery)
                });
            })))
        ]);
    }

    // ---- the share card

    const CARD_LANG_KEY = 'perf-sentinel:card-lang';
    const CARD_FIX_PAGE = 5;
    const CARD_FREE = [['title', 48], ['team', 48]];

    /** The card form and its answers. It outlives a recap read: the filters above only move its scope. */
    function cardState() {
        if (!state.card) {
            // An explicit choice, or null: the card then follows the Hub's language.
            let lang = null;
            try {
                lang = localStorage.getItem(CARD_LANG_KEY);
            } catch (error) {
                // A comfort only: the Hub's language it is.
            }
            state.card = {
                open: false, angle: 'curve', kind: 'month', from: null, signature: null, label: null,
                remember: false, rememberError: null, title: '', team: '', lang: lang, country: null, format: 'png',
                options: null, optionsPath: null, curveRefusal: null, showAllFixes: false,
                preview: null, stale: false, busy: null, renderError: null, fieldErrors: {},
                attach: null, attachBusy: false, busyNote: null, attachNote: null,
                keys: undefined, keysError: null, keysBusy: false, revoking: null, revokeDate: ''
            };
        }
        return state.card;
    }

    function cardToday() {
        return new Date().toISOString().slice(0, 10);
    }

    /** The picked period, or the recap filter's when it is closed for this kind, else the last closed one. */
    function cardPeriodFrom(card, route) {
        if (card.from) return card.from;
        const last = PSL.lastClosedPeriod(card.kind, cardToday());
        const filtered = route.at ? PSL.cardPeriodStart(card.kind, route.at) : null;
        return filtered && filtered <= last ? filtered : last;
    }

    function cardOptionsReady(card) {
        return card.options && card.options !== 'loading' && !card.options.error ? card.options : null;
    }

    /** The fix a story tells: the one picked when the period still lists it, else the default. */
    function cardFix(card) {
        const options = cardOptionsReady(card);
        if (!options) return null;
        return options.fixes.find(function (fix) {
            return fix.signature === card.signature;
        }) || options.fixes.find(function (fix) {
            return fix.signature === options.default_signature;
        }) || options.fixes[0] || null;
    }

    function cardLabel(card) {
        const fix = cardFix(card);
        return card.label !== null ? card.label : (fix && fix.label) || '';
    }

    function cardForm(card, route) {
        const fix = cardFix(card);
        return {
            angle: card.angle, period: card.kind, at: cardPeriodFrom(card, route), environment: route.environment,
            signature: fix ? fix.signature : null, label: cardLabel(card), title: card.title, team: card.team,
            lang: PSL.cardLang(card.lang), format: card.format,
            equivalence_country: PSL.cardCountryWire(PSL.cardCountry(card.country, cardHubCountry(card)))
        };
    }

    /** The Hub's equivalence country, once the options are read. */
    function cardHubCountry(card) {
        const options = cardOptionsReady(card);
        return options ? options.equivalence_country : undefined;
    }

    function cardFileName(card, route) {
        return 'perf-sentinel-card-' + card.kind + '-' + cardPeriodFrom(card, route) + '.' + card.format;
    }

    /** The curve's own refusal, from its last render: the period before is not whole. */
    function cardCurveRefusal(card) {
        const refusal = card.angle === 'curve' && card.curveRefusal && card.curveRefusal.path === card.optionsPath
            ? card.curveRefusal : null;
        if (!refusal) return null;
        const options = cardOptionsReady(card);
        return refusal.code && options ? PSL.cardRefusalText(refusal.code, card.kind, options.period.from) : refusal.text;
    }

    /** Why no card can be made with these choices, before any field: null when one can. */
    function cardBlocked(card) {
        const options = cardOptionsReady(card);
        if (!options) return 'options';
        if (options.refusal) return 'refused';
        // A story tells one printable fix, a collective counts every confirmed one: both need one.
        if (card.angle === 'story' && options.fixes.length === 0) return 'no_fix';
        if (card.angle === 'collective' && !options.confirmed_fixes_count) return 'no_fix';
        return cardCurveRefusal(card) ? 'previous' : null;
    }

    /** The free fields this angle shows, by their card.json name. */
    function cardFields(card) {
        return card.angle === 'story' && cardFix(card) && !cardBlocked(card) ? ['label', 'title', 'team'] : ['title', 'team'];
    }

    function cardFieldValue(card, field) {
        return field === 'label' ? cardLabel(card) : card[field];
    }

    /** The error under a free field: the browser's own check first, then the engine's 422. */
    function cardFieldError(card, field) {
        return PSL.cardFieldProblem(cardFieldValue(card, field)) || card.fieldErrors[field] || null;
    }

    function cardInvalid(card) {
        return cardFields(card).some(function (field) {
            return cardFieldError(card, field) !== null;
        });
    }

    function cardPlace(card, management) {
        const fix = cardFix(card);
        return management || !fix ? t('card.place.this_action') : fix.endpoint;
    }

    /** A render that keeps the focus, and the caret, on the control that had them. */
    function cardRender() {
        const active = document.activeElement;
        const id = active && active.id;
        const caret = active && typeof active.selectionStart === 'number' ? [active.selectionStart, active.selectionEnd] : null;
        render();
        const again = id ? document.getElementById(id) : null;
        if (!again) return;
        again.focus({preventScroll: true});
        if (caret) again.setSelectionRange(caret[0], caret[1]);
    }

    /** A choice that changes the card: a shown preview no longer matches it. */
    function cardMarkStale(card) {
        if (card.preview) card.stale = true;
        card.renderError = null;
    }

    function cardPick(change) {
        const card = cardState();
        Object.assign(card, change);
        cardMarkStale(card);
        cardRender();
    }

    /** A format whose preview renders the same file, PNG and JPEG XL, leaves the preview current. */
    function cardPickFormat(format) {
        const card = cardState();
        const same = PSL.previewFormat(card.format) === PSL.previewFormat(format);
        card.format = format;
        if (same) card.renderError = null;
        else cardMarkStale(card);
        cardRender();
    }

    function loadCardOptions(card, path) {
        card.optionsPath = path;
        card.options = 'loading';
        getJson(path).then(function (options) {
            if (card.optionsPath === path) card.options = options;
        }).catch(function (error) {
            if (card.optionsPath === path) card.options = {error: error.status || 0};
        }).finally(function () {
            if (card.optionsPath === path && currentScreen() === 'recap') cardRender();
        });
    }

    function loadCardKeys(card) {
        card.keys = 'loading';
        getJson('/api/cards/keys').then(function (keys) {
            card.keys = keys;
        }).catch(function (error) {
            // Not an administrator after all, or cards off: the server decides, the panel goes.
            card.keys = error.status === 403 || error.status === 404 ? null : 'error';
        }).finally(function () {
            if (currentScreen() === 'recap') cardRender();
        });
    }

    /** The section under the reports: shown when this Hub signs cards, a banner when its engine cannot. */
    function recapCardPanel(route) {
        const cards = state.status && state.status.cards;
        if (cards !== true && cards !== false) return null;
        const card = cardState();
        const management = route.view === 'pilotage';
        const section = el('section', {class: 'recap-section share', 'aria-labelledby': 'recap-h-share'}, [
            ruledOverline(t('card.overline')),
            recapHeading('recap-h-share', PSL.CARD_COPY.title, PSL.CARD_COPY.hint, PSL.CARD_COPY.help, t('card.topic'))
        ]);
        if (cards === false) {
            section.appendChild(el('div', {class: 'banner share-off', 'data-tone': 'muted', role: 'status'},
                [infoGlyph(16), el('div', {text: PSL.CARD_COPY.engineOff})]));
            return section;
        }
        const admin = state.access === null || state.access.admin;
        if (admin && card.keys === undefined) loadCardKeys(card);
        if (!card.open) {
            const create = el('button', {
                class: 'pill-button pill-primary', type: 'button', id: 'card-create', 'aria-expanded': 'false'
            }, [cardImageGlyph(), el('span', {text: PSL.CARD_COPY.create})]);
            create.addEventListener('click', function () {
                card.open = true;
                renderFocus('#card-angle-' + card.angle);
            });
            section.appendChild(el('div', {class: 'share-closed'}, [el('p', {text: PSL.CARD_COPY.closed}), create]));
        } else {
            const path = PSL.cardOptionsPath(cardForm(card, route));
            if (card.optionsPath !== path) loadCardOptions(card, path);
            const form = el('form', {class: 'share-form', 'aria-label': t('card.form'), novalidate: ''}, [
                cardAngleGroup(card),
                cardPeriodGroup(card, route, management),
                card.angle === 'story' ? cardFixGroup(card, management) : null,
                cardWordsGroup(card, management),
                cardFormatGroup(card),
                cardActions(card, route)
            ]);
            form.addEventListener('submit', function (event) {
                event.preventDefault();
            });
            section.appendChild(el('p', {class: 'share-intro', text: PSL.CARD_COPY.intro}));
            section.appendChild(el('div', {class: 'share-grid'}, [form, cardPreviewAside(card, route, management, admin)]));
        }
        if (admin && card.keys !== null && card.keys !== undefined) section.appendChild(cardKeysPanel(card));
        return section;
    }

    function cardImageGlyph() {
        return svg([
            ['rect', {x: '4', y: '3.5', width: '16', height: '17', rx: '2'}],
            ['path', {d: 'M4 16l4.5-4.5 4 4 2.5-2.5L20 18'}],
            ['circle', {cx: '15', cy: '8.5', r: '1.5'}]
        ], 14);
    }

    function cardSegmented(label, name, choices, current, onPick) {
        const group = el('div', {class: 'segmented segmented-sm', role: 'radiogroup', 'aria-label': label},
            choices.map(function (choice) {
                const button = el('button', {
                    type: 'button', role: 'radio', id: 'card-' + name + '-' + choice[0],
                    'aria-checked': choice[0] === current ? 'true' : 'false', text: choice[1]
                });
                button.addEventListener('click', function () {
                    if (choice[0] !== current) onPick(choice[0]);
                });
                return button;
            }));
        return radioKeys(group, 'card-' + name);
    }

    function cardLegend(text, small) {
        return el('legend', {class: 'share-legend'}, [document.createTextNode(text),
            small ? el('small', {text: small}) : null]);
    }

    function cardAngleGroup(card) {
        return el('fieldset', {class: 'share-group'}, [
            cardLegend(t('card.legend.angle')),
            el('div', {class: 'choice-list'}, PSL.CARD_ANGLES.map(function (angle) {
                const input = el('input', {
                    type: 'radio', name: 'card-angle', id: 'card-angle-' + angle, value: angle,
                    'aria-describedby': 'card-angle-line-' + angle
                });
                input.checked = card.angle === angle;
                input.addEventListener('change', function () {
                    cardPick({angle: angle});
                });
                return el('label', {class: 'choice'}, [input,
                    el('span', {class: 'choice-name', text: PSL.CARD_ANGLE_NAMES[angle]}),
                    el('span', {class: 'choice-line', id: 'card-angle-line-' + angle, text: PSL.CARD_ANGLE_LINES[angle]})]);
            }))
        ]);
    }

    function cardStepButton(card, from, delta, enabled) {
        const button = el('button', {
            type: 'button', id: delta < 0 ? 'card-period-back' : 'card-period-next',
            'aria-label': t((delta < 0 ? 'card.step.back.' : 'card.step.next.') + card.kind),
            disabled: enabled ? null : 'disabled'
        }, [svg([['path', {d: delta < 0 ? 'M15 6l-6 6 6 6' : 'M9 6l6 6-6 6'}]], 13)]);
        button.addEventListener('click', function () {
            cardPick({from: PSL.stepPeriod(card.kind, from, delta)});
        });
        return button;
    }

    function cardPeriodGroup(card, route, management) {
        const from = cardPeriodFrom(card, route);
        const name = PSL.cardPeriodName(card.kind, from);
        const last = !PSL.canStepForward(card.kind, from, cardToday());
        const group = el('fieldset', {class: 'share-group'}, [
            cardLegend(t('card.legend.period'), t('card.legend.period.small')),
            el('div', {class: 'period-row'}, [
                cardSegmented(t('card.length'), 'kind', PSL.CARD_PERIODS.map(function (kind) {
                    return [kind, PSL.CARD_PERIOD_LABELS[kind]];
                }), card.kind, function (kind) {
                    cardPick({kind: kind, from: null});
                }),
                el('div', {class: 'period-step', role: 'group', 'aria-label': t('card.period_aria')}, [
                    cardStepButton(card, from, -1, true),
                    el('output', {'aria-live': 'polite', text: name.charAt(0).toUpperCase() + name.slice(1)}),
                    cardStepButton(card, from, 1, !last)
                ]),
                last ? el('span', {class: 'period-hint', text: t('card.step.last.' + card.kind)}) : null
            ])
        ]);
        const options = card.options;
        if (options === 'loading' || options === null) {
            group.appendChild(el('p', {class: 'period-main', role: 'status', text: t('card.period.reading')}));
            return group;
        }
        if (options.error) {
            group.appendChild(el('div', {class: 'banner', 'data-tone': 'crit', role: 'alert'},
                [critGlyph(16), el('div', {text: PSL.CARD_COPY.optionsFailed})]));
            return group;
        }
        const previous = PSL.cardPeriodName(card.kind, PSL.stepPeriod(card.kind, from, -1));
        const range = PSL.cardRangeText(options.period.from, options.period.to);
        group.appendChild(el('p', {
            class: 'period-main',
            text: card.angle === 'story' ? range : t('card.compared', {range: range, previous: previous})
        }));
        group.appendChild(cardEnvironmentLine(options.environment, management));
        if (options.refusal) {
            group.appendChild(cardWarn([
                el('b', {text: t('card.refused', {period: PSL.cardPeriodName(card.kind, options.period.from)}) + ' '}),
                document.createTextNode(PSL.cardRefusalText(options.refusal, card.kind, options.period.from))]));
            return group;
        }
        const curve = cardCurveRefusal(card);
        if (curve) {
            const story = el('button', {class: 'pill-button pill-sm', type: 'button', text: t('card.make.story')});
            story.addEventListener('click', function () {
                cardPick({angle: 'story'});
            });
            const collective = el('button', {class: 'pill-button pill-sm', type: 'button', text: t('card.make.collective')});
            collective.addEventListener('click', function () {
                cardPick({angle: 'collective'});
            });
            group.appendChild(cardWarn([document.createTextNode(curve),
                el('div', {class: 'banner-actions'}, [story, collective])]));
            return group;
        }
        group.appendChild(cardPeriodFacts(card, options, previous));
        if (card.angle === 'collective' && !options.confirmed_fixes_count) group.appendChild(cardNoFix());
        return group;
    }

    function cardWarn(children) {
        return el('div', {class: 'banner', 'data-tone': 'warn', role: 'alert'}, [warningGlyph(16), el('div', {}, children)]);
    }

    /** The environment in mono, as the recap filter names it. */
    function cardEnvironmentLine(environment, management) {
        const text = PSL.cardEnvironmentLine(environment, management ? 'pilotage' : 'tech');
        if (!environment) return el('p', {class: 'period-env', text: text});
        const at = text.indexOf(environment);
        return el('p', {class: 'period-env'}, [document.createTextNode(text.slice(0, at)),
            el('b', {text: environment}), document.createTextNode(text.slice(at + environment.length))]);
    }

    function cardPeriodFacts(card, options, previous) {
        const days = Math.round((Date.parse(options.period.to) - Date.parse(options.period.from)) / 86400000) + 1;
        const where = options.sources.length > 1
            ? t('card.list', {items: options.sources.slice(0, -1).join(', '), last: options.sources[options.sources.length - 1]})
            : options.sources[0];
        const facts = [['ok', where ? t('card.facts.recorded.at', {days: days, where: where}) : t('card.facts.recorded', {days: days})]];
        const before = {previous: previous.charAt(0).toUpperCase() + previous.slice(1)};
        if (card.angle === 'collective' && !options.previous_whole) {
            facts.push(['info', t('card.facts.previous.not_whole', before)]);
        } else if (card.angle === 'collective' && !options.previous_complete) {
            facts.push(['info', t('card.facts.previous.not_complete', before)]);
        } else if (card.angle !== 'story' && options.previous_complete) {
            facts.push(['ok', t('card.facts.previous.whole', before)]);
        }
        if (options.restart_days > 0) {
            facts.push(['info', t('card.facts.restart.' + PSL.plural(options.restart_days), {count: options.restart_days})]);
        }
        if (options.sampled_permille < 1000) {
            const permille = options.sampled_permille;
            facts.push(['info', t('card.facts.sampled', {percent: PSL.pct(permille / 1000, permille % 10 ? 1 : 0)})]);
        }
        return el('ul', {class: 'period-facts', 'aria-label': t('card.facts.aria')}, facts.map(function (fact) {
            return el('li', {'data-tone': fact[0]}, [
                fact[0] === 'ok' ? svg(RECAP_STATE_GLYPH.confirmed, 14) : infoGlyph(14),
                el('span', {text: fact[1]})
            ]);
        }));
    }

    function cardFixGroup(card, management) {
        const options = cardOptionsReady(card);
        const group = el('fieldset', {class: 'share-group'}, [cardLegend(t('card.legend.fix'), options
            ? t('card.legend.fix.small', {period: PSL.cardPeriodName(card.kind, options.period.from)}) : null)]);
        if (!options || options.refusal) {
            group.appendChild(el('p', {class: 'preview-note', text: PSL.CARD_COPY.fixesWait}));
            return group;
        }
        if (options.fixes.length === 0) {
            group.appendChild(cardNoFix());
            return group;
        }
        const chosen = cardFix(card);
        const at = options.fixes.indexOf(chosen);
        const shown = card.showAllFixes || at >= CARD_FIX_PAGE ? options.fixes : options.fixes.slice(0, CARD_FIX_PAGE);
        group.appendChild(el('ul', {class: 'fix-list'}, shown.map(function (fix, index) {
            return el('li', {}, [cardFixRow(card, fix, index, fix === chosen, options, management)]);
        })));
        if (shown.length < options.fixes.length) {
            const more = el('button', {
                class: 'pill-button pill-sm fix-more', type: 'button', text: t('card.fix.all', {count: options.fixes.length})
            });
            more.addEventListener('click', function () {
                card.showAllFixes = true;
                renderFocus('#card-fix-' + CARD_FIX_PAGE);
            });
            group.appendChild(more);
        }
        return group;
    }

    function cardNoFix() {
        return el('div', {class: 'banner', 'data-tone': 'muted', role: 'status'},
            [infoGlyph(16), el('div', {text: PSL.CARD_COPY.noFix})]);
    }

    function cardFixRow(card, fix, index, checked, options, management) {
        const input = el('input', {type: 'radio', name: 'card-fix', id: 'card-fix-' + index, value: fix.signature});
        input.checked = checked;
        input.addEventListener('change', function () {
            cardPick({signature: fix.signature, label: null, remember: false, rememberError: null});
        });
        const line = PSL.cardFixLine(fix, PSL.cardPeriodName(card.kind, PSL.stepPeriod(card.kind, options.period.from, -1)));
        const pill = fix.signature === options.default_signature && fix.before_fix_ops !== null;
        const meta = [
            management ? el('span', {text: t('card.fix.in', {service: fix.service})}) : null,
            el('span', {
                text: t('card.fix.done', {day: PSL.day(new Date(fix.done_at_ms).toISOString().slice(0, 10), {withYear: true})})
            }),
            el('span', {}, [line.figure ? el('b', {text: line.figure}) : null, document.createTextNode(line.text)])
        ];
        return el('label', {class: 'fix'}, [
            input,
            el('span', {class: 'fix-plain'}, [document.createTextNode(PSL.recapPlain(fix) + ' '),
                pill ? el('span', {class: 'fix-badge', text: PSL.CARD_COPY.defaultPill}) : null]),
            management ? null : el('span', {class: 'fix-head'}, [
                el('span', {
                    class: 'chip', 'data-type': fix.finding_type,
                    text: PSL.FINDING_TYPE_LABEL[fix.finding_type] || fix.finding_type
                }),
                el('span', {class: 'table-strong', text: fix.service}),
                el('span', {class: 'recap-endpoint', text: fix.endpoint})
            ]),
            el('span', {class: 'fix-meta'}, meta)
        ]);
    }

    /** Typing redraws only when what the screen says changes: an error, the stale state, the remember box. */
    function cardTyped(card, field, value, count, max) {
        const view = function () {
            const fix = cardFix(card);
            return JSON.stringify([card.stale, card.renderError, cardFields(card).map(function (name) {
                return cardFieldError(card, name);
            }), PSL.rememberState(cardLabel(card), fix && fix.label, '')]);
        };
        const before = view();
        if (field === 'label') card.label = value;
        else card[field] = value;
        delete card.fieldErrors[field];
        cardMarkStale(card);
        if (view() !== before) cardRender();
        else count.textContent = value.length + ' / ' + max;
    }

    function cardTextField(card, field, label, max, placeholder, notes, wide) {
        const id = 'card-' + field;
        const value = cardFieldValue(card, field);
        const error = cardFieldError(card, field);
        const count = el('span', {class: 'tfield-count', 'aria-hidden': 'true', text: value.length + ' / ' + max});
        const described = [error ? id + '-error' : null].concat(notes.map(function (note) {
            return note.id;
        })).filter(Boolean).join(' ');
        const input = el('input', {
            class: 'tinput', id: id, type: 'text', maxlength: String(max), placeholder: placeholder,
            spellcheck: 'false', 'aria-invalid': error ? 'true' : null, 'aria-describedby': described || null
        });
        input.value = value;
        input.addEventListener('input', function () {
            cardTyped(card, field, input.value, count, max);
        });
        return el('div', {class: 'tfield' + (wide ? ' wide' : '')}, [
            el('label', {class: 'tfield-label', for: id}, [el('span', {text: label}), count]),
            input,
            error ? el('p', {class: 'tfield-error', id: id + '-error', text: error}) : null
        ].concat(notes));
    }

    function cardWordsGroup(card, management) {
        const grid = el('div', {class: 'free-grid'});
        if (cardFields(card).indexOf('label') >= 0) grid.appendChild(cardLabelField(card, management));
        CARD_FREE.forEach(function (free) {
            grid.appendChild(cardTextField(card, free[0], t('card.free.' + free[0]), free[1],
                t('card.free.' + free[0] + '.placeholder'), [], false));
        });
        return el('fieldset', {class: 'share-group'}, [
            cardLegend(t('card.legend.words'), t('card.legend.words.small')),
            grid,
            el('p', {class: 'free-note', text: PSL.CARD_COPY.freeNote})
        ]);
    }

    function cardLabelField(card, management) {
        const fix = cardFix(card);
        const place = cardPlace(card, management);
        const at = t('card.label.prefilled.at').split('{place}');
        const note = el('p', {class: 'tfield-note', id: 'card-label-note'}, fix.label
            ? (management
                ? [document.createTextNode(t('card.label.prefilled'))]
                : [document.createTextNode(at[0]), el('code', {text: place}), document.createTextNode(at[1])])
            : [document.createTextNode(PSL.CARD_COPY.labelHint)]);
        const field = cardTextField(card, 'label', t('card.label'), 80, t('card.label.placeholder'), [note], true);
        const allowed = PSL.canRememberLabel(state.sources);
        const remember = PSL.rememberState(cardLabel(card), fix.label, place);
        const box = el('input', {
            type: 'checkbox', id: 'card-remember',
            disabled: allowed && remember.enabled ? null : 'disabled',
            'aria-describedby': allowed ? null : 'card-remember-why'
        });
        box.checked = allowed && remember.enabled && card.remember;
        box.addEventListener('change', function () {
            card.remember = box.checked;
        });
        field.appendChild(el('label', {class: 'checkbox'}, [box, el('span', {text: remember.text})]));
        if (!allowed) {
            field.appendChild(el('p', {class: 'tfield-note', id: 'card-remember-why', text: PSL.CARD_COPY.rememberDenied}));
        }
        if (card.rememberError) {
            field.appendChild(el('p', {class: 'tfield-error', role: 'alert', text: card.rememberError}));
        }
        return field;
    }

    function cardFormatGroup(card) {
        const country = PSL.cardCountry(card.country, cardHubCountry(card));
        return el('fieldset', {class: 'share-group'}, [
            cardLegend(t('card.legend.card')),
            el('div', {class: 'card-opts'}, [
                el('div', {class: 'card-opt'}, [el('span', {class: 'card-opt-label', text: t('card.lang')}),
                    cardSegmented(t('card.lang'), 'lang', [['en', 'English'], ['fr', 'Fran\u00e7ais']],
                        PSL.cardLang(card.lang), function (lang) {
                            store('localStorage', CARD_LANG_KEY, lang);
                            cardPick({lang: lang});
                        })]),
                el('div', {class: 'card-opt card-opt-format'}, [el('span', {class: 'card-opt-label', text: t('card.format_label')}),
                    cardSegmented(t('card.format_label'), 'format', PSL.CARD_FORMATS.map(function (format) {
                        return [format, PSL.CARD_FORMAT_LABELS[format]];
                    }), card.format, cardPickFormat)]),
                el('div', {class: 'card-opt card-opt-country'}, [el('span', {class: 'card-opt-label', text: t('card.country')}),
                    cardSegmented(t('card.country'), 'country', PSL.CARD_COUNTRIES.map(function (country) {
                        return [country, t('card.country.' + country)];
                    }), country, function (picked) {
                        cardPick({country: picked});
                    })])
            ]),
            country === 'other' ? el('p', {class: 'free-note', text: t('card.country_note.other')}) : null,
            card.format === 'pdf' ? el('p', {class: 'free-note', text: PSL.CARD_COPY.carouselNote}) : null,
            PSL.CARD_FORMAT_NOTES[card.format] ? el('p', {class: 'free-note', text: PSL.CARD_FORMAT_NOTES[card.format]})
                : null
        ]);
    }

    function cardActions(card, route) {
        const invalid = cardInvalid(card);
        const off = cardBlocked(card) || invalid || card.busy ? 'disabled' : null;
        const preview = el('button', {class: 'pill-button pill-primary', type: 'button', id: 'card-preview', disabled: off},
            [svg([['path', {d: 'M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z'}],
                ['circle', {cx: '12', cy: '12', r: '2.8'}]], 14),
            el('span', {
                text: t(card.busy === 'preview' ? 'card.action.rendering'
                    : card.stale && !cardBlocked(card) ? 'card.action.preview_again' : 'card.action.preview')
            })]);
        preview.addEventListener('click', function () {
            cardPreview(route);
        });
        const download = el('button', {class: 'pill-button', type: 'button', id: 'card-download', disabled: off},
            [downloadGlyph(), el('span', {
                text: t(card.busy === 'download' ? 'card.action.downloading'
                    : card.format === 'pdf' ? 'card.action.download_pdf' : 'card.action.download_image')
            })]);
        download.addEventListener('click', function () {
            cardDownload(route);
        });
        return el('div', {class: 'share-actions'}, [
            preview, download,
            el('span', {class: 'share-file', text: cardFileName(card, route)}),
            cardBusyNote(card.busyNote),
            invalid ? el('p', {class: 'share-why', text: PSL.CARD_COPY.fieldWhy}) : null,
            card.renderError ? el('p', {class: 'recap-alert', role: 'alert', text: card.renderError}) : null
        ]);
    }

    function cardPreviewAside(card, route, management, admin) {
        const blocked = cardBlocked(card);
        const format = card.preview ? card.preview.format : card.format;
        const pdf = format === 'pdf';
        const shown = cardPreviewShown(card, blocked);
        const aside = el('aside', {class: 'share-preview', 'aria-label': t('card.preview'), tabindex: '-1'}, [
            el('div', {class: 'preview-head'}, [document.createTextNode(t('card.preview')),
                el('span', {text: t(pdf ? 'card.preview.pdf' : 'card.preview.image')})])
        ]);
        if (shown && card.stale) {
            aside.appendChild(el('div', {class: 'banner', 'data-tone': 'info', role: 'status'},
                [infoGlyph(16), el('div', {text: PSL.CARD_COPY.stale})]));
        }
        aside.appendChild(cardPreviewFrame(card, blocked, pdf));
        const options = cardOptionsReady(card);
        const domain = options && options.org.domain;
        if (shown) cardPreviewProof(card, format, domain).forEach(function (line) {
            aside.appendChild(line);
        });
        if (options) aside.appendChild(cardAttachBlock(card, route, domain, management, admin));
        return aside;
    }

    function cardPreviewShown(card, blocked) {
        return !!card.preview && !blocked && card.busy !== 'preview';
    }

    /** The frame holds one of three things: the render in progress, the card, or why there is none yet. */
    function cardPreviewFrame(card, blocked, pdf) {
        const frame = el('div', {class: 'preview-frame'});
        if (card.busy === 'preview') {
            frame.appendChild(el('div', {class: 'preview-loading', role: 'status', text: PSL.CARD_COPY.rendering}));
        } else if (cardPreviewShown(card, blocked)) {
            if (card.stale) frame.setAttribute('data-stale', '');
            frame.appendChild(cardPreviewCard(card, pdf));
        } else {
            frame.appendChild(cardPreviewEmpty(blocked));
        }
        return frame;
    }

    function cardPreviewCard(card, pdf) {
        return pdf
            ? el('iframe', {class: 'card-preview-pdf', src: card.preview.url, title: t('card.preview.pdf_title')})
            : el('img', {class: 'card-preview', src: card.preview.url, alt: t('card.preview.image_alt')});
    }

    function cardPreviewEmpty(blocked) {
        const slim = blocked && blocked !== 'options';
        return el('div', {class: 'preview-empty' + (slim ? ' slim' : '')}, [
            cardImageGlyph(),
            el('span', {text: slim ? PSL.CARD_COPY.previewBlocked : PSL.CARD_COPY.previewEmpty})
        ]);
    }

    /** What the shown card carries: the note on the file, and the level its proof reaches. */
    function cardPreviewProof(card, format, domain) {
        const attached = !!(card.attach && card.attach.result && card.attach.result.integrity === 'ok'
            && card.attach.result.content !== 'modified' && card.attach.result.level === 2);
        const level = PSL.levelLine(attached, domain);
        const at = level.indexOf(': ') + 2;
        return [
            el('p', {class: 'preview-note', text: PSL.CARD_COPY[format + 'Note']}),
            el('p', {class: 'proof-line'}, [document.createTextNode(level.slice(0, at)), el('b', {text: level.slice(at)})])
        ];
    }

    function cardAttachBlock(card, route, domain, management, admin) {
        if (!domain) return el('div', {class: 'attach'}, [el('p', {class: 'muted', text: PSL.CARD_COPY.noDomain})]);
        const button = el('button', {
            class: 'pill-button pill-sm', type: 'button', id: 'card-attach',
            disabled: card.attachBusy || cardBlocked(card) || cardInvalid(card) ? 'disabled' : null,
            text: t(card.attachBusy ? 'card.attach.checking' : 'card.attach.verify')
        });
        button.addEventListener('click', function () {
            cardCheckAttachment(route);
        });
        const box = el('div', {class: 'attach'}, [
            el('div', {class: 'attach-head'}, [el('p', {class: 'attach-title', text: t('card.attach.title', {domain: domain})}), button]),
            el('p', {class: 'attach-contact', text: PSL.cardContactText(domain, management ? 'pilotage' : 'tech')}),
            cardBusyNote(card.attachNote)
        ]);
        const answer = card.attach;
        if (answer && answer.error) {
            box.appendChild(el('div', {class: 'banner', 'data-tone': 'crit', role: 'alert'},
                [critGlyph(16), el('div', {text: answer.error})]));
        } else if (answer) {
            const result = answer.result;
            const said = PSL.attachmentText(result, admin);
            let tone = 'info';
            if (result.integrity !== 'ok' || result.content === 'modified') tone = 'crit';
            else if (result.level === 2) tone = 'ok';
            else if (said.title) tone = 'warn';
            const glyph = {crit: critGlyph(16), ok: svg(RECAP_STATE_GLYPH.confirmed, 16), warn: warningGlyph(16)}[tone];
            box.appendChild(el('div', {class: 'banner', 'data-tone': tone, role: 'status', title: said.title},
                [glyph || infoGlyph(16), el('div', {text: said.text})]));
            const intact = PSL.contentLine(result);
            if (intact) box.appendChild(el('p', {class: 'attach-content', text: intact}));
            const figures = PSL.signedFigureLines(result.figures);
            if (figures.length) box.appendChild(cardSignedFigures(figures));
        }
        return box;
    }

    /** The figures as signed, exact, for the person to hold against the preview before sharing. */
    function cardSignedFigures(figures) {
        return el('section', {class: 'card-signed', 'aria-labelledby': 'card-signed-title'}, [
            el('p', {class: 'card-signed-title', id: 'card-signed-title', text: t('card.signed.title')}),
            el('p', {class: 'card-signed-note', text: t('card.signed.note')}),
            el('dl', {class: 'card-signed-figures'}, figures.flatMap(function (figure) {
                return [el('dt', {text: figure.label}), el('dd', {text: figure.value})];
            }))
        ]);
    }

    /** Under the button that waits: another card holds the engine, the Hub asks again by itself. */
    function cardBusyNote(text) {
        return text ? el('div', {class: 'banner card-busy', 'data-tone': 'info', role: 'status'},
            [infoGlyph(16), el('div', {text: text})]) : null;
    }

    /**
     * A POST of the card form; a file comes back as a Blob, a refusal as the Hub's own detail. A busy render
     * gate is told through onWait and asked once more after its Retry-After; that second answer stands.
     */
    function cardPost(path, body, read, onWait) {
        const send = function () {
            return fetch(path, {
                method: 'POST',
                headers: {
                    'content-type': 'application/json',
                    accept: 'application/json, image/png, image/svg+xml, image/jxl, application/pdf'
                },
                body: JSON.stringify(body)
            }).then(signInOn401);
        };
        return send().then(function (response) {
            const busy = onWait ? PSL.cardBusy(response.status, response.headers.get('Retry-After')) : null;
            if (!busy) return response;
            onWait(busy.text);
            return new Promise(function (resolve) {
                setTimeout(resolve, busy.delayMs);
            }).then(send);
        }).then(function (response) {
            if (response.ok) {
                return read === 'blob' ? response.blob().then(function (blob) {
                    return {blob: blob};
                }) : response.json().then(function (result) {
                    return {result: result};
                });
            }
            return response.json().catch(function () {
                return null;
            }).then(function (payload) {
                return {
                    status: response.status,
                    field: payload && typeof payload.field === 'string' ? payload.field : null,
                    code: payload && typeof payload.code === 'string' ? payload.code : null,
                    detail: PSL.problemText(payload, response.status, t('card.refused_status', {status: response.status}))
                };
            });
        }).catch(function () {
            return {status: 0, field: null, detail: t('card.no_answer')};
        });
    }

    /** A refusal lands where it belongs: under its field, on the curve's period, or under the buttons. */
    function cardFailure(card, failure) {
        if (failure.field) card.fieldErrors[failure.field] = failure.detail;
        else if (failure.status === 422 && card.angle === 'curve' && PSL.isPreviousRefusal(failure.detail, failure.code)) {
            card.curveRefusal = {path: card.optionsPath, code: failure.code, text: failure.detail};
        } else card.renderError = failure.detail;
    }

    /** Remembers the typed name for the fix's endpoint first, when the box asks for it. */
    function cardRememberLabel(card) {
        const fix = cardFix(card);
        const label = cardLabel(card).trim();
        if (card.angle !== 'story' || !card.remember || !fix || !PSL.rememberState(label, fix.label, '').enabled) {
            return Promise.resolve();
        }
        return recapSend('PUT', '/api/labels', {service: fix.service, endpoint: fix.endpoint, label: label})
            .then(function (failure) {
                card.rememberError = failure;
                if (failure) return;
                fix.label = label;
                card.label = null;
                card.remember = false;
            });
    }

    function cardMake(route, busy, then) {
        const card = cardState();
        if (card.busy) return;
        const body = PSL.cardBody(cardForm(card, route));
        if (busy === 'preview' && body.format) body.format = PSL.previewFormat(body.format);
        card.busy = busy;
        card.renderError = null;
        cardRender();
        cardRememberLabel(card).then(function () {
            return cardPost('/api/cards/render', body, 'blob', function (text) {
                card.busyNote = text;
                if (currentScreen() === 'recap') cardRender();
            });
        }).then(function (answer) {
            card.busy = null;
            card.busyNote = null;
            if (answer.blob) then(card, answer.blob, body);
            else cardFailure(card, answer);
            if (currentScreen() === 'recap') cardRender();
        });
    }

    /** Preview renders the form as it is, a JPEG XL as its PNG; a phone then scrolls to the card. */
    function cardPreview(route) {
        cardMake(route, 'preview', function (card, blob, body) {
            if (card.preview) URL.revokeObjectURL(card.preview.url);
            card.preview = {url: URL.createObjectURL(blob), format: body.format};
            card.stale = false;
            setTimeout(function () {
                const aside = document.querySelector('.share-preview');
                if (!aside || !matchMedia('(max-width: 900px)').matches) return;
                const still = matchMedia('(prefers-reduced-motion: reduce)').matches;
                aside.scrollIntoView({behavior: still ? 'auto' : 'smooth', block: 'start'});
                aside.focus({preventScroll: true});
            }, 0);
        });
    }

    /** Download renders the card again from the form, never from the preview on screen. */
    function cardDownload(route) {
        const name = cardFileName(cardState(), route);
        cardMake(route, 'download', function (card, blob) {
            PSL.saveBlob(blob, name);
        });
    }

    function cardCheckAttachment(route) {
        const card = cardState();
        if (card.attachBusy) return;
        card.attachBusy = true;
        card.attach = null;
        cardRender();
        cardPost('/api/cards/attachment', PSL.cardBody(cardForm(card, route)), 'json', function (text) {
            card.attachNote = text;
            if (currentScreen() === 'recap') cardRender();
        }).then(function (answer) {
            card.attachBusy = false;
            card.attachNote = null;
            card.attach = answer.result ? {result: answer.result} : {error: answer.detail};
            if (currentScreen() === 'recap') cardRender();
        });
    }

    // ---- the signing key, for administrators

    function cardKeysPanel(card) {
        const panel = el('details', {class: 'key-panel'}, [
            el('summary', {}, [svg([['path', {d: 'M9 6l6 6-6 6'}]], 12),
                document.createTextNode(PSL.CARD_COPY.keysTitle), el('small', {text: t('card.keys.small')})])
        ]);
        if (state.panelOpen['card-keys']) panel.open = true;
        panel.addEventListener('toggle', function () {
            state.panelOpen['card-keys'] = panel.open;
        });
        const body = el('div', {class: 'key-body'});
        panel.appendChild(body);
        cardKeysFill(card, body);
        return panel;
    }

    function cardKeysFill(card, body) {
        if (card.keys === 'loading') {
            body.appendChild(el('p', {class: 'key-p', role: 'status', text: t('card.keys.reading')}));
            return;
        }
        if (card.keys === 'error') {
            body.appendChild(el('p', {class: 'recap-alert', role: 'alert', text: t('card.keys.failed')}));
            return;
        }
        const options = cardOptionsReady(card);
        const domain = card.keys.record_name ? card.keys.record_name.replace(/^_perf-sentinel\./, '') : null;
        body.appendChild(el('div', {}, [
            el('dl', {class: 'recap-kv'}, [
                options ? el('dt', {text: t('card.keys.organisation')}) : null,
                options ? el('dd', {class: 'ui', text: options.org.name}) : null,
                el('dt', {text: t('card.keys.domain')}),
                el('dd', {text: domain || t('card.keys.not_set')})
            ]),
            el('p', {class: 'key-p', style: 'margin-top:8px', text: PSL.CARD_COPY.keysOrigin})
        ]));
        if (card.keysError) body.appendChild(el('p', {class: 'recap-alert', role: 'alert', text: card.keysError}));
        if (card.keys.keys.length === 0) {
            body.appendChild(el('p', {class: 'key-p', text: PSL.CARD_COPY.keysNone}));
            return;
        }
        if (domain) {
            body.appendChild(el('div', {}, [
                el('p', {class: 'key-h', text: t('card.keys.attach', {domain: domain})}),
                el('p', {class: 'key-p', text: PSL.CARD_COPY.keysPublish}),
                cardCopyLine(card.keys.record_name, card.keys.record_name, t('card.keys.copy_record'))
            ]));
        }
        body.appendChild(el('ul', {class: 'key-list'}, card.keys.keys.map(function (key) {
            return cardKeyRow(card, key, domain);
        })));
        if (domain) body.appendChild(cardKeysFile(card, domain));
        body.appendChild(cardKeyRotate(card));
    }

    function cardKeyRotate(card) {
        const rotate = el('button', {
            class: 'pill-button pill-sm', type: 'button', id: 'card-rotate',
            disabled: card.keysBusy ? 'disabled' : null, text: t(card.keysBusy ? 'card.keys.working' : 'card.keys.rotate')
        });
        rotate.addEventListener('click', function () {
            cardKeyWrite('/api/cards/keys/rotate', {});
        });
        return el('div', {class: 'key-rotate'}, [rotate, el('p', {text: PSL.CARD_COPY.rotate})]);
    }

    /** The .well-known file, indented on screen when it parses and copied as served. */
    function cardKeysFile(card, domain) {
        let shown = card.keys.well_known;
        try {
            shown = JSON.stringify(JSON.parse(card.keys.well_known), null, 2);
        } catch (error) {
            // Shown as served.
        }
        const at = t('card.keys.file.at').split('{url}');
        return el('div', {}, [
            el('p', {class: 'key-h', text: t('card.keys.file')}),
            el('p', {class: 'key-p'}, [document.createTextNode(at[0]),
                el('code', {text: 'https://' + domain + '/.well-known/perf-sentinel-key'}),
                document.createTextNode(at[1] + ' ' + PSL.CARD_COPY.keysFile)]),
            cardCopyLine(shown, card.keys.well_known, t('card.keys.copy_file'))
        ]);
    }

    /** A line to paste: shown as `text`, copied as `raw`, the .well-known file indented only on screen. */
    function cardCopyLine(text, raw, label) {
        const code = el('pre', {class: 'terminal-code', tabindex: '0', text: text});
        const word = el('span', {text: t('card.copy')});
        const button = el('button', {class: 'pill-button', type: 'button', 'aria-label': label}, [copyGlyph(), word]);
        button.addEventListener('click', function () {
            const copy = globalThis.isSecureContext && navigator.clipboard
                ? navigator.clipboard.writeText(raw).then(function () {
                    return true;
                }, function () {
                    return false;
                })
                : writeClipboard(code);
            copy.then(function (copied) {
                word.textContent = t(copied ? 'card.copied' : 'card.copy_by_hand');
                if (copied) button.setAttribute('data-copied', 'true');
                setTimeout(function () {
                    word.textContent = t('card.copy');
                    button.removeAttribute('data-copied');
                }, 2000);
            });
        });
        return el('div', {class: 'copy-line'}, [code, button]);
    }

    function cardKeyRow(card, key, domain) {
        let stateName = 'retired';
        if (key.revoked) stateName = 'revoked';
        else if (key.current) stateName = 'current';
        const row = el('li', {class: 'key-row'}, [
            el('div', {class: 'key-row-head'}, [
                el('span', {class: 'key-fp', text: key.fingerprint}),
                el('span', {class: 'key-state', 'data-state': stateName, text: t('card.key.state.' + stateName)}),
                el('span', {
                    class: 'key-dates',
                    text: t('card.key.dates' + (key.to ? '.to' : '') + (key.revoked ? '.revoked' : ''),
                        {from: key.from, to: key.to, revoked: key.revoked})
                })
            ]),
            domain ? cardCopyLine(key.dns, key.dns, t('card.key.copy_dns', {fingerprint: key.fingerprint})) : null
        ]);
        if (!key.revoked) row.appendChild(card.revoking === key.fingerprint ? cardRevokeConfirm(card, key, domain)
            : cardRevokeOpen(card, key));
        return row;
    }

    function cardRevokeOpen(card, key) {
        const open = el('button', {
            class: 'pill-button pill-sm pill-crit', type: 'button', id: 'card-revoke-' + key.fingerprint, text: t('card.revoke')
        });
        open.addEventListener('click', function () {
            card.revoking = key.fingerprint;
            card.revokeDate = '';
            renderFocus('#card-leak-date');
        });
        return el('div', {class: 'key-revoke'}, [el('span', {text: t('card.revoke.leaked')}), open]);
    }

    /** No default date: it is published as the first day the key may have leaked. */
    function cardRevokeConfirm(card, key, domain) {
        const date = el('input', {
            class: 'recap-input', id: 'card-leak-date', type: 'date', required: '', min: key.from, max: cardToday()
        });
        date.value = card.revokeDate;
        const revoke = el('button', {
            class: 'pill-button pill-sm pill-crit', type: 'button', id: 'card-revoke-confirm',
            disabled: card.revokeDate && !card.keysBusy ? null : 'disabled', text: t('card.revoke')
        });
        const cancel = el('button', {class: 'recap-link', type: 'button', text: t('card.revoke.cancel')});
        date.addEventListener('change', function () {
            card.revokeDate = date.value;
            cardRender();
        });
        revoke.addEventListener('click', function () {
            cardKeyWrite('/api/cards/keys/revoke', {fingerprint: key.fingerprint, revoked: card.revokeDate});
        });
        cancel.addEventListener('click', function () {
            card.revoking = null;
            renderFocus('#card-revoke-' + key.fingerprint);
        });
        const said = card.revokeDate ? PSL.revokeText(card.revokeDate, key.current, domain) : null;
        return el('div', {class: 'key-confirm'}, [
            el('div', {}, [el('label', {for: 'card-leak-date', text: PSL.CARD_COPY.leaked}), date]),
            said ? el('p', {role: 'alert', style: 'margin:0', text: said.alert}) : null,
            said ? el('p', {class: 'key-confirm-step', text: said.step}) : null,
            el('div', {}, [revoke, cancel])
        ]);
    }

    function cardKeyWrite(path, body) {
        const card = cardState();
        if (card.keysBusy) return;
        card.keysBusy = true;
        card.keysError = null;
        cardRender();
        recapSend('POST', path, body).then(function (failure) {
            card.keysBusy = false;
            card.keysError = failure;
            if (!failure) {
                card.revoking = null;
                // A new or revoked key changes what the check page reads: check the attachment again.
                card.attach = null;
            }
            loadCardKeys(card);
        });
    }

    // ---------------------------------------------------- screen: disclosures

    const LEVEL_LABELS = {
        'none': 'disc.level.none',
        'hash-only': 'disc.level.hash_only',
        'signed': 'disc.level.signed',
        'signed-with-attestation': 'disc.level.signed_attestation',
        'audited': 'disc.level.audited'
    };
    const RENDER_REASONS = {
        unsupported_engine: 'disc.render.unsupported_engine',
        unsupported_schema_version: 'disc.render.unsupported_schema_version',
        unsupported_characters: 'disc.render.unsupported_characters',
        pdf_export_failed: 'disc.render.pdf_export_failed',
        output_unwritable: 'disc.render.output_unwritable',
        timeout: 'disc.render.timeout',
        content_hash_mismatch: 'disc.render.content_hash_mismatch',
        binding_mismatch: 'disc.render.binding_mismatch'
    };
    const DISCLOSURE_FILES = [
        ['report.pdf', 'disc.file.pdf', 'pdf'],
        ['report.json', 'disc.file.report', 'report'],
        ['attestation.intoto.jsonl', 'disc.file.attestation', 'attestation'],
        ['bundle.sig', 'disc.file.bundle', 'bundle'],
        ['SHA256SUMS', 'disc.file.sums', 'report']
    ];

    function loadDisclosures() {
        state.disclosures = null;
        state.disclosuresError = false;
        render();
        getJson('/api/disclosures').then(function (list) {
            state.disclosures = list;
        }).catch(function () {
            state.disclosuresError = true;
            state.disclosures = [];
        }).finally(function () {
            if (currentScreen() === 'disclosures') render();
        });
    }

    /** The detail, and the report itself for what the API leaves to it: applications, method, disclaimers. */
    function loadDisclosure(id) {
        if (!state.disclosure || state.disclosure.disclosure.id !== id) {
            state.disclosure = null;
            state.disclosureReport = null;
        }
        state.disclosureError = false;
        render();
        Promise.all([
            getJson('/api/disclosures/' + id),
            fetch('/api/disclosures/' + id + '/files/report.json', {headers: {accept: 'application/json'}})
                .then(signInOn401)
                .then(function (response) {
                    return response.ok ? response.json() : null;
                })
                .catch(function () {
                    return null;
                })
        ]).then(function (results) {
            state.disclosure = results[0];
            state.disclosureReport = results[1];
        }).catch(function () {
            state.disclosureError = true;
        }).finally(function () {
            if (currentScreen() === 'disclosure') render();
        });
    }

    function sendDisclosure(path, method, body) {
        return fetch(path, {
            method: method,
            headers: {'content-type': 'application/json', accept: 'application/json'},
            body: JSON.stringify(body || {})
        }).then(signInOn401).then(function (response) {
            return response.json().catch(function () {
                return null;
            }).then(function (payload) {
                return {ok: response.ok, status: response.status, payload: payload};
            });
        }, function () {
            return {ok: false, status: 0, payload: null};
        });
    }

    function periodDays(period) {
        return Math.round((Date.parse(period.to) - Date.parse(period.from)) / 86400000) + 1;
    }

    function shortHash(hash) {
        const value = String(hash);
        return value.length > 20 ? value.slice(0, 11) + '…' + value.slice(-4) : value;
    }

    function checkGlyph() {
        return svg([['path', {d: 'M5 12.5l4.2 4.2L19 7'}]], 11);
    }

    /** A code the Hub or the report sends (intent, delivery state, refusal) in the shown language, else as sent. */
    function enumWord(kind, value) {
        const key = 'disc.' + kind + '.' + value;
        return PSL.hasText(key) ? t(key) : value;
    }

    function levelLabel(level) {
        return LEVEL_LABELS[level] ? t(LEVEL_LABELS[level]) : level;
    }

    /** What the report declares about itself, dashed: the Hub cannot check it. */
    function levelChip(level) {
        return el('span', {class: 'chip chip-declared disc-level', text: levelLabel(level) || t('disc.level.undeclared')});
    }

    /** What this Hub recomputed, solid. Anything it did not check reads as muted text, never a mark. */
    function checkChip(label, outcome) {
        if (outcome === 'checked') return el('span', {class: 'chip disc-checked'}, [el('span', {text: label}), checkGlyph()]);
        if (outcome === 'declared') return el('span', {
            class: 'chip chip-declared disc-level',
            text: t('disc.chip.declared', {label: label})
        });
        return el('span', {
            class: 'chip disc-unchecked',
            text: outcome === 'absent' ? t('disc.chip.absent', {label: label}) : t('disc.chip.not_checked', {label: label})
        });
    }

    function disclosureChips(item) {
        const chips = [
            el('span', {class: 'chip', text: enumWord('intent', item.intent)}),
            el('span', {class: 'chip', text: enumWord('confidentiality', item.confidentiality)}),
            levelChip(item.integrity_level)
        ];
        if (item.pdf === 'rendered') {
            chips.push(checkChip(t('disc.chip.hash'), 'checked'));
            if (item.files.attestation) chips.push(checkChip(t('disc.chip.bound'), 'checked'));
        } else {
            chips.push(el('span', {class: 'chip disc-unchecked', text: t('disc.chip.unchecked')}));
        }
        if (item.confidentiality === 'internal') chips.push(el('span', {
            class: 'chip chip-declared',
            text: t('disc.chip.never_shared')
        }));
        return chips;
    }

    function figure(label, value, unit, sub) {
        return el('div', {class: 'disc-figure'}, [
            el('span', {class: 'disc-figure-label', text: label}),
            el('span', {class: 'disc-figure-value'}, [
                el('span', {text: value}),
                unit ? el('span', {class: 'disc-figure-unit', text: unit}) : null
            ]),
            sub ? el('span', {class: 'disc-figure-sub', text: sub}) : null
        ]);
    }

    function fixed(value, digits) {
        return value == null ? t('unit.na') : PSL.decimal(Number(value).toFixed(digits));
    }

    /** Runtime coverage on a track, with the official 75 % as a notch and the fill orange under it. */
    function coverageBar(coverage) {
        const notch = PSL.coverageNotch(coverage);
        if (!notch) return el('span', {class: 'disc-figure-sub', text: t('disc.coverage.not_reported')});
        const fill = el('span', {class: 'disc-track-fill'});
        fill.style.width = Math.min(100, notch.percent) + '%';
        return el('div', {
            class: 'disc-track', 'data-below': notch.below ? 'true' : null, role: 'img',
            'aria-label': t(notch.below ? 'disc.coverage.aria.below' : 'disc.coverage.aria.over', {percent: notch.percent})
        }, [
            fill,
            el('span', {class: 'disc-track-notch'}),
            el('span', {class: 'disc-track-mark', text: t('disc.coverage.mark')})
        ]);
    }

    function disclosureFigures(figures) {
        const f = figures || {};
        const coverage = PSL.coverageNotch(f.coverage);
        return el('div', {class: 'disc-figures'}, [
            figure(t('disc.figure.energy'), fixed(f.energy_kwh, 2), 'kWh', null),
            figure(t('disc.figure.carbon'), fixed(f.carbon_kgco2eq, 2), 'kgCO₂eq',
                f.carbon_kgco2eq == null ? null
                    : t('disc.figure.bracket', {low: fixed(f.carbon_kgco2eq / 2, 2), high: fixed(f.carbon_kgco2eq * 2, 2)})),
            figure(t('disc.figure.avoidable'), f.avoidable_ratio == null ? t('unit.na') : Math.round(f.avoidable_ratio * 100),
                t('disc.figure.avoidable.unit'), null),
            el('div', {class: 'disc-figure'}, [
                el('span', {class: 'disc-figure-label', text: t('disc.figure.coverage')}),
                el('span', {class: 'disc-figure-value'}, [
                    el('span', {text: coverage ? String(coverage.percent) : t('unit.na')}),
                    coverage ? el('span', {class: 'disc-figure-unit', text: '%'}) : null
                ]),
                coverageBar(f.coverage)
            ])
        ]);
    }

    function fact(label, value) {
        return el('span', {class: 'disc-fact'}, [
            el('span', {class: 'disc-fact-label', text: label}),
            el('span', {class: 'disc-fact-value', text: value})
        ]);
    }

    function ago(ms) {
        return t('disc.ago', {duration: PSL.dur(Date.now() - ms)});
    }

    function disclosureFacts(item) {
        return el('div', {class: 'disc-facts'}, [
            fact(t('disc.fact.published'), ago(item.published_at_ms)),
            item.signature_added_at_ms ? fact(t('disc.fact.signed_copy'), ago(item.signature_added_at_ms)) : null,
            fact('pdf', item.pdf === 'rendered' ? t('disc.fact.pdf.rendered') : t('disc.fact.pdf.not_rendered')),
            el('span', {class: 'disc-fact disc-hash', title: item.content_hash, text: shortHash(item.content_hash)})
        ]);
    }

    function disclosureFileButton(id, name, primary, status) {
        const button = el('button', {type: 'button', class: 'pill-button pill-sm' + (primary ? ' pill-primary' : '')},
            [primary ? downloadGlyph() : null, el('span', {text: primary ? t('disc.download_pdf') : name})]);
        button.addEventListener('click', function () {
            button.disabled = true;
            PSL.downloadAs('/api/disclosures/' + id + '/files/' + name, name).then(function (saved) {
                button.disabled = false;
                if (status) status.textContent = t(saved ? 'disc.download.sent' : 'disc.download.missing', {name: name});
            });
        });
        return button;
    }

    function disclosureBanners(item) {
        const banners = [];
        if (item.withdrawn) {
            banners.push(el('div', {class: 'banner', 'data-tone': 'crit'}, [critGlyph(16), el('div', {
                text: t('disc.banner.withdrawn', {
                    duration: PSL.dur(Date.now() - item.withdrawn.at_ms),
                    by: item.withdrawn.by || t('disc.banner.withdrawn.someone'),
                    // A reason typed as a sentence brings its own full stop, and the banner adds one.
                    reason: (item.withdrawn.reason || '').replace(/\.+\s*$/, '') || t('disc.banner.withdrawn.no_reason')
                })
            })]));
        } else if (item.intent === 'official' && (item.integrity_level === 'hash-only' || item.integrity_level === 'none')) {
            banners.push(el('div', {class: 'banner', 'data-tone': 'warn'}, [warningGlyph(16), el('div', {
                text: t('disc.banner.unsigned')
            })]));
        }
        if (item.pdf !== 'rendered') {
            // A render again the engine refused: the files are suspect, not merely unchecked.
            const refused = item.render_reason === 'content_hash_mismatch' || item.render_reason === 'binding_mismatch';
            const reason = RENDER_REASONS[item.render_reason] ? t(RENDER_REASONS[item.render_reason])
                : item.render_reason || t('disc.render.none');
            const text = el('div', {text: t(refused ? 'disc.banner.refused' : 'disc.banner.not_checked', {reason: reason})});
            banners.push(el('div', {
                class: 'banner',
                'data-tone': refused ? 'crit' : 'info'
            }, refused ? [critGlyph(16), text] : [text]));
        }
        return banners;
    }

    function disclosureCard(item) {
        const status = el('span', {class: 'terminal-status', role: 'status'});
        const actions = [
            item.files.pdf ? disclosureFileButton(item.id, 'report.pdf', true, status) : null,
            disclosureFileButton(item.id, 'report.json', false, status),
            item.files.attestation ? disclosureFileButton(item.id, 'attestation.intoto.jsonl', false, status) : null,
            item.files.bundle ? disclosureFileButton(item.id, 'bundle.sig', false, status) : null,
            el('a', {
                class: 'pill-button pill-sm disc-open',
                href: '#/disclosures/' + item.id
            }, [el('span', {text: t('disc.details')})]),
            status
        ];
        return el('article', {class: 'card disc-card', 'data-withdrawn': item.withdrawn ? 'true' : null}, [
            el('header', {class: 'disc-card-head'}, [
                el('h2', {class: 'disc-card-title'}, [
                    el('a', {href: '#/disclosures/' + item.id, text: PSL.periodLabel(item.period)})
                ]),
                el('span', {
                    class: 'disc-dates',
                    text: t('disc.card.dates', {from: PSL.day(item.period.from, {withYear: true}), to: PSL.day(item.period.to, {withYear: true}),
                        days: periodDays(item.period)})
                }),
                el('span', {class: 'disc-chips'}, disclosureChips(item))
            ]),
            el('div', {class: 'disc-card-body'}, disclosureBanners(item).concat([disclosureFigures(item.figures), disclosureFacts(item)])),
            el('div', {class: 'disc-actions'}, actions)
        ]);
    }

    function railView(list) {
        const rail = PSL.periodRail(list, Date.now());
        if (rail.length === 0) return null;
        return el('ol', {class: 'disc-rail', 'aria-label': t('disc.rail.label')}, rail.map(function (cell) {
            const caption = cell.state === 'published' ? levelLabel(cell.level)
                : cell.state === 'current' ? t('disc.rail.closes', {days: cell.daysLeft})
                    : t('disc.rail.' + cell.state);
            const body = [el('span', {
                class: 'disc-rail-label',
                text: cell.label
            }), el('span', {class: 'disc-rail-caption', text: caption})];
            return el('li', {class: 'disc-rail-cell', 'data-state': cell.state}, cell.id
                ? [el('a', {href: '#/disclosures/' + cell.id}, body)]
                : body);
        }));
    }

    function railLegend() {
        return el('div', {class: 'disc-legend'}, [
            el('span', {class: 'disc-legend-label', text: t('disc.legend.declared')}),
            levelChip('hash-only'), levelChip('signed'), levelChip('signed-with-attestation'),
            el('span', {class: 'disc-legend-label', text: t('disc.legend.checked')}),
            checkChip(t('disc.chip.hash'), 'checked'), checkChip(t('disc.chip.bound'), 'checked'),
            el('p', {class: 'disc-legend-note', text: t('disc.legend.note')})
        ]);
    }

    function publishPanel() {
        return terminalBlock({
            head: t('disc.publish.head'),
            sub: t('disc.publish.sub'),
            text: PSL.publishCommand(location.origin),
            copyLabel: t('disc.publish.copy'),
            notes: [t('disc.publish.note')]
        });
    }

    function renderDisclosuresScreen() {
        const section = el('section', {class: 'disc'}, [
            ruledOverline(t('disc.overline')),
            el('h1', {class: 'page-title', text: t('disc.title')}),
            el('p', {class: 'page-sub', text: t('disc.sub')})
        ]);
        if (state.disclosures === null) {
            section.appendChild(el('div', {class: 'card skeleton', style: 'height:220px'}));
            return section;
        }
        if (state.disclosuresError) {
            section.appendChild(el('div', {class: 'banner', 'data-tone': 'crit'}, [
                critGlyph(16),
                el('div', {text: t('disc.list.error')})
            ]));
            return section;
        }
        if (state.disclosures.length === 0) {
            section.appendChild(el('div', {class: 'empty-state'}, [
                el('p', {class: 'empty-title', text: t('disc.empty.title')}),
                el('p', {text: t('disc.empty.body')})
            ]));
            section.appendChild(publishPanel());
            return section;
        }
        section.appendChild(el('div', {class: 'disc-rail-wrap'}, [
            el('span', {class: 'disc-legend-label', text: t('disc.rail.head')}),
            railView(state.disclosures)
        ]));
        section.appendChild(railLegend());
        state.disclosures.forEach(function (item) {
            section.appendChild(disclosureCard(item));
        });
        section.appendChild(publishPanel());
        return section;
    }

    // -------------------------------------------- screen: one disclosure

    function detailPanel(head, sub, children) {
        return el('section', {class: 'card disc-panel'}, [
            el('div', {class: 'disc-panel-head'}, [
                el('h2', {class: 'overline', text: head}),
                sub ? el('p', {class: 'disc-panel-sub', text: sub}) : null
            ])
        ].concat(children));
    }

    function simpleTable(columns, rows, right) {
        return el('div', {class: 'sources-wrap'}, [el('table', {class: 'table'}, [
            el('thead', {}, [el('tr', {}, columns.map(function (name) {
                return el('th', {
                    scope: 'col',
                    text: name,
                    'data-align': (right || []).indexOf(name) >= 0 ? 'right' : null
                });
            }))]),
            el('tbody', {}, rows)
        ])]);
    }

    /** The carbon figure on a log scale between half and twice itself: the ~2× bracket the method states. */
    function carbonBracket(carbon) {
        if (carbon == null || carbon <= 0) return null;
        return el('div', {class: 'disc-bracket'}, [
            el('span', {class: 'disc-figure-label', text: t('disc.bracket.label')}),
            el('div', {
                class: 'disc-bracket-track', role: 'img',
                'aria-label': t('disc.bracket.aria', {value: fixed(carbon, 2), low: fixed(carbon / 2, 2), high: fixed(carbon * 2, 2)})
            }, [
                el('span', {class: 'disc-bracket-band'}),
                el('span', {class: 'disc-bracket-dot'})
            ]),
            el('div', {class: 'disc-bracket-scale'}, [
                el('span', {text: fixed(carbon / 2, 2)}),
                el('span', {class: 'disc-bracket-mid', text: fixed(carbon, 2) + ' kgCO₂eq'}),
                el('span', {text: fixed(carbon * 2, 2)})
            ])
        ]);
    }

    function applicationsPanel(report, figures) {
        const applications = report && Array.isArray(report.applications) ? report.applications : [];
        const hosts = PSL.hostRows(report);
        if (applications.length === 0 && hosts.length === 0) return null;
        // Over the report's total, as the engine's PDF takes them: it holds the hosts, which no
        // application carries, and energy no service was attributed. The rows only stand in
        // for a report without one.
        const reported = Number(report.aggregate && report.aggregate.total_energy_kwh);
        const total = reported > 0 ? reported : applications.reduce(function (sum, app) {
            return sum + (Number(app.energy_kwh) || 0);
        }, 0) + hosts.reduce(function (sum, host) {
            return sum + host.energy_kwh;
        }, 0);

        function shareCell(kwh) {
            const share = total > 0 ? (Number(kwh) || 0) / total : 0;
            const bar = el('span', {class: 'disc-share-fill'});
            bar.style.width = Math.round(share * 100) + '%';
            return el('td', {}, [el('span', {class: 'disc-share'}, [el('span', {class: 'disc-share-track'}, [bar]),
                el('span', {class: 'table-mono', text: t('disc.apps.share', {percent: Math.round(share * 100)})})])]);
        }

        const rows = applications.map(function (app) {
            return el('tr', {}, [
                el('td', {class: 'table-strong', text: app.display_name || app.service_name}),
                el('td', {
                    class: 'table-mono',
                    'data-align': 'right',
                    text: app.total_requests == null ? '' : PSL.num(Number(app.total_requests))
                }),
                shareCell(app.energy_kwh),
                el('td', {class: 'table-mono', 'data-align': 'right', text: fixed(app.energy_kwh, 2)}),
                el('td', {class: 'table-mono', 'data-align': 'right', text: fixed(app.carbon_kgco2eq, 2)}),
                el('td', {
                    class: 'table-mono',
                    'data-align': 'right',
                    text: app.anti_patterns_detected_count == null ? '' : String(app.anti_patterns_detected_count)
                })
            ]);
        });
        hosts.forEach(function (host) {
            rows.push(el('tr', {}, [
                el('td', {class: 'table-strong'}, [document.createTextNode(host.label),
                    host.partial ? document.createTextNode(' ') : null,
                    host.partial ? el('span', {
                        class: 'disc-host-note',
                        text: t('disc.apps.host.partial', {windows: host.partial.windows, of: host.partial.of})
                    }) : null]),
                el('td', {}),
                shareCell(host.energy_kwh),
                el('td', {class: 'table-mono', 'data-align': 'right', text: PSL.hostKwh(host.energy_kwh)}),
                el('td', {class: 'table-mono', 'data-align': 'right', text: fixed(host.carbon_kgco2eq, 2)}),
                el('td', {})
            ]));
        });
        const excluded = report.scope_manifest && Array.isArray(report.scope_manifest.applications_excluded)
            ? report.scope_manifest.applications_excluded : [];
        excluded.forEach(function (app) {
            const reason = app && app.reason;
            rows.push(el('tr', {}, [
                el('td', {}, [el('span', {
                    class: 'table-strong',
                    text: (app && (app.service_name || app.name)) || String(app)
                }), document.createTextNode(' '),
                    el('span', {class: 'chip chip-declared', text: t('disc.apps.excluded')})]),
                // The report's own reason is engine text, and stays as the report wrote it.
                reason ? engineText(el('td', {class: 'table-muted', colspan: '5', text: reason}))
                    : el('td', {class: 'table-muted', colspan: '5', text: t('disc.apps.excluded.reason')})
            ]));
        });
        const declared = figures && figures.applications_declared;
        const measured = figures && figures.applications_measured;
        const coverage = declared != null && measured != null
            ? t('disc.apps.measured', {measured: measured, declared: declared}) : null;
        const hostNote = hosts.length === 0 ? null : t(hosts.length > 1 ? 'disc.apps.hosts.both'
            : hosts[0].label === t('disc.host.database') ? 'disc.apps.hosts.database' : 'disc.apps.hosts.broker');
        const heads = ['application', 'requests', 'share', 'kwh', 'carbon', 'anti_patterns'].map(function (id) {
            return t('disc.apps.col.' + id);
        });
        return detailPanel(t('disc.apps.head'), [coverage, hostNote].filter(Boolean).join(' ') || null,
            [simpleTable(heads, rows, [heads[1], heads[3], heads[4], heads[5]])]);
    }

    function integrityPanel(detail) {
        const rows = PSL.checkRows(detail).map(function (row) {
            const outcome = row.outcome === 'checked' ? checkChip(t('disc.outcome.checked'), 'checked')
                : row.outcome === 'declared' ? el('span', {class: 'chip chip-declared disc-level', text: t('disc.outcome.declared')})
                    : el('span', {
                        class: 'chip disc-unchecked',
                        text: PSL.hasText('disc.outcome.' + row.outcome) ? t('disc.outcome.' + row.outcome) : row.outcome
                    });
            return el('tr', {}, [
                el('td', {class: 'table-strong', text: row.claim}),
                el('td', {class: 'table-mono', text: row.by}),
                el('td', {}, [outcome])
            ]);
        });
        const item = detail.disclosure;
        return detailPanel(t('disc.integrity.head'), t('disc.integrity.sub'),
            [
                simpleTable([t('disc.integrity.col.claim'), t('disc.integrity.col.by'), t('disc.integrity.col.outcome')], rows),
                el('dl', {class: 'disc-defs'}, [
                    el('dt', {text: 'content_hash'}), el('dd', {class: 'table-mono', text: item.content_hash}),
                    el('dt', {text: 'report_uuid'}), el('dd', {class: 'table-mono', text: item.report_uuid}),
                    el('dt', {text: t('disc.integrity.signer')}), el('dd', {
                        class: 'table-mono',
                        text: detail.expected_identity ? detail.expected_identity + ' · ' + detail.expected_issuer
                            : t('disc.integrity.signer.none')
                    })
                ]),
                terminalBlock({
                    head: t('disc.verify.head'),
                    sub: t('disc.verify.sub'),
                    text: PSL.verifyCommand(detail),
                    copyLabel: t('disc.verify.copy'),
                    notes: [t('disc.verify.note')]
                })
            ]);
    }

    function filesPanel(item) {
        const status = el('span', {class: 'terminal-status', role: 'status'});
        const rows = DISCLOSURE_FILES.filter(function (file) {
            return item.files[file[2]];
        }).map(function (file) {
            return el('tr', {}, [
                el('td', {class: 'table-strong', text: file[0]}),
                el('td', {class: 'table-mono', text: t(file[1])}),
                el('td', {'data-align': 'right'}, [disclosureFileButton(item.id, file[0], false, status)])
            ]);
        });
        return detailPanel(t('disc.files.head'), t('disc.files.sub'),
            [simpleTable([t('disc.files.col.file'), t('disc.files.col.role'), ''], rows), status]);
    }

    function linkState(link) {
        return link.state === 'active' ? t('disc.link.active', {duration: PSL.dur(link.expires_at_ms - Date.now())})
            : link.state === 'revoked' ? t('disc.link.revoked', {by: link.revoked_by})
                : link.state === 'withdrawn' ? t('disc.link.withdrawn') : t('disc.link.expired');
    }

    function shareLinksPanel(detail, manage) {
        const item = detail.disclosure;
        const rows = detail.share_links.map(function (link) {
            const revoke = manage && link.state === 'active'
                ? disclosureActionButton(t('disc.links.revoke'), function () {
                    return sendDisclosure('/api/disclosures/' + item.id + '/share-links/' + link.id, 'DELETE');
                }, t('disc.links.revoked'))
                : null;
            return el('tr', {}, [
                el('td', {class: 'table-mono', text: link.created_by}),
                el('td', {class: 'table-mono', text: ago(link.created_at_ms)}),
                el('td', {class: 'table-mono', text: linkState(link)}),
                el('td', {class: 'table-mono', 'data-align': 'right', text: String(link.downloads)}),
                el('td', {
                    class: 'table-mono',
                    text: link.last_used_at_ms ? ago(link.last_used_at_ms) : t('disc.links.never')
                }),
                el('td', {'data-align': 'right'}, [revoke])
            ]);
        });
        const children = [];
        if (state.disclosureLink && state.disclosureLink.id === item.id) children.push(terminalBlock({
            head: t('disc.links.new.head'),
            sub: t('disc.links.new.sub'),
            text: state.disclosureLink.pdfUrl + '\n\nperf-sentinel verify-hash --url ' + state.disclosureLink.reportUrl,
            copyLabel: t('disc.links.new.copy')
        }));
        const heads = ['created_by', 'created', 'state', 'uses', 'last_use'].map(function (id) {
            return t('disc.links.col.' + id);
        });
        children.push(rows.length
            ? simpleTable(heads.concat(['']), rows, [heads[3]])
            : el('p', {
                class: 'sources-note',
                text: t(item.confidentiality === 'internal' ? 'disc.links.internal' : 'disc.links.none')
            }));
        return detailPanel(t('disc.links.head'), t('disc.links.sub'), children);
    }

    function deliveriesPanel(detail, manage) {
        if (detail.deliveries.length === 0) return null;
        const item = detail.disclosure;
        const rows = detail.deliveries.map(function (delivery) {
            const retry = manage && delivery.state !== 'delivered'
                ? disclosureActionButton(t('disc.deliveries.retry'), function () {
                    return sendDisclosure('/api/disclosures/' + item.id + '/deliveries/' + delivery.id + '/retry', 'POST');
                }, t('disc.deliveries.retried'))
                : null;
            return el('tr', {}, [
                el('td', {class: 'table-strong', text: delivery.webhook}),
                el('td', {class: 'table-mono', text: delivery.event}),
                el('td', {
                    class: 'table-mono',
                    text: delivery.last_attempt_ms ? ago(delivery.last_attempt_ms) : t('disc.deliveries.not_yet')
                }),
                el('td', {}, [el('span', {
                    class: 'status-pill', 'data-status': delivery.state === 'delivered' ? 'succeeded'
                        : delivery.state === 'failed' ? 'failed' : 'queued',
                    text: t('disc.deliveries.outcome.' + PSL.plural(delivery.attempts), {
                        state: enumWord('delivery', delivery.state),
                        status: delivery.last_status ? ' · ' + delivery.last_status : '',
                        attempts: delivery.attempts
                    })
                })]),
                el('td', {'data-align': 'right'}, [retry])
            ]);
        });
        const heads = ['webhook', 'event', 'last_attempt', 'outcome'].map(function (id) {
            return t('disc.deliveries.col.' + id);
        });
        return detailPanel(t('disc.deliveries.head'), t('disc.deliveries.sub'), [simpleTable(heads.concat(['']), rows)]);
    }

    function methodPanel(report) {
        if (!report) return null;
        const methodology = report.methodology || {};
        const disclaimers = report.notes && Array.isArray(report.notes.disclaimers) ? report.notes.disclaimers : [];
        const defs = [
            ['specification', methodology.sci_specification],
            ['conformance', methodology.conformance],
            ['patterns', Array.isArray(methodology.enabled_patterns) ? methodology.enabled_patterns.join(', ') : null],
            ['carbon_intensity', methodology.calibration_inputs && methodology.calibration_inputs.carbon_intensity_source]
        ].filter(function (pair) {
            return pair[1];
        });
        return detailPanel(t('disc.method.head'), t('disc.method.sub'), [
            el('dl', {class: 'disc-defs'}, defs.flatMap(function (pair) {
                return [el('dt', {text: t('disc.method.' + pair[0])}), el('dd', {class: 'table-mono', text: String(pair[1])})];
            })),
            // Copied verbatim from the report: engine text, as the engine wrote it.
            disclaimers.length ? engineText(el('ol', {class: 'disc-disclaimers'}, disclaimers.map(function (text) {
                return el('li', {text: String(text)});
            }))) : null
        ]);
    }

    /** A button that runs one write, says what came of it, and reloads the detail. */
    function disclosureActionButton(label, run, done, primary) {
        const button = el('button', {
            type: 'button', class: 'pill-button pill-sm' + (primary ? ' pill-primary' : ''),
            text: label
        });
        button.addEventListener('click', function () {
            if (state.disclosureBusy) return;
            state.disclosureBusy = true;
            button.disabled = true;
            run().then(function (outcome) {
                state.disclosureBusy = false;
                state.disclosureNote = outcome && outcome.ok ? done
                    : (outcome && outcome.payload && (outcome.payload.detail
                        ? PSL.problemText(outcome.payload, outcome.status) : enumWord('refusal', outcome.payload.reason)))
                    || t('disc.action.refused', {status: outcome ? outcome.status : 0});
                loadDisclosure(PSL.readDisclosureId(location.hash));
            });
        });
        return button;
    }

    function withdrawForm(item) {
        const reason = el('textarea', {
            class: 'disc-reason', rows: '2', maxlength: '500', 'aria-label': t('disc.withdraw.reason.label'),
            placeholder: t('disc.withdraw.reason.placeholder')
        });
        const submit = disclosureActionButton(t('disc.withdraw.submit'), function () {
            if (!reason.value.trim()) return Promise.resolve({
                ok: false,
                status: 400,
                payload: {detail: t('disc.withdraw.reason.missing')}
            });
            return sendDisclosure('/api/disclosures/' + item.id + '/withdraw', 'POST', {reason: reason.value.trim()});
        }, t('disc.withdraw.done'));
        return el('div', {class: 'disc-withdraw'}, [
            el('p', {text: t('disc.withdraw.body')}),
            reason,
            el('div', {class: 'disc-actions'}, [submit])
        ]);
    }

    function detailActions(detail, manage) {
        const item = detail.disclosure;
        const status = el('span', {class: 'terminal-status', role: 'status'});
        const buttons = [item.files.pdf ? disclosureFileButton(item.id, 'report.pdf', true, status) : null];
        if (manage && !item.withdrawn && item.confidentiality !== 'internal') {
            buttons.push(disclosureActionButton(t('disc.action.new_link'), function () {
                return sendDisclosure('/api/disclosures/' + item.id + '/share-links', 'POST', {}).then(function (outcome) {
                    if (outcome.ok) {
                        const base = location.origin + outcome.payload.path;
                        state.disclosureLink = {
                            id: item.id,
                            pdfUrl: base + 'report.pdf',
                            reportUrl: base + 'report.json'
                        };
                    }
                    return outcome;
                });
            }, t('disc.action.new_link.done')));
        }
        if (manage && item.pdf !== 'rendered') {
            buttons.push(disclosureActionButton(t('disc.action.render'), function () {
                return sendDisclosure('/api/disclosures/' + item.id + '/render', 'POST', {});
            }, t('disc.action.render.done')));
        }
        if (manage && !item.withdrawn) {
            const toggle = el('button', {
                type: 'button', class: 'pill-button pill-sm',
                'aria-expanded': state.withdrawOpen ? 'true' : 'false', text: t('disc.action.withdraw')
            });
            toggle.addEventListener('click', function () {
                state.withdrawOpen = !state.withdrawOpen;
                render();
            });
            buttons.push(toggle);
        }
        buttons.push(status);
        return el('div', {class: 'disc-actions'}, buttons);
    }

    function renderDisclosureScreen() {
        const back = el('a', {
            class: 'pill-button pill-sm disc-back',
            href: '#/disclosures'
        }, [el('span', {text: t('disc.back')})]);
        const section = el('section', {class: 'disc'}, [back]);
        if (state.disclosureError) {
            section.appendChild(el('div', {class: 'banner', 'data-tone': 'crit'}, [critGlyph(16),
                el('div', {text: t('disc.detail.error')})]));
            return section;
        }
        const detail = state.disclosure;
        if (!detail) {
            section.appendChild(el('div', {class: 'card skeleton', style: 'height:320px'}));
            return section;
        }
        const item = detail.disclosure;
        // The Hub decides: an action needs disclosures.manage and a caller it can name.
        const manage = detail.can_manage === true;
        const intent = 'disc.detail.intent.' + item.intent;
        [
            ruledOverline(t('disc.detail.overline', {period: PSL.periodLabel(item.period)})),
            el('h1', {
                class: 'page-title', text: t('disc.detail.title', {organisation: item.organisation,
                    from: PSL.day(item.period.from, {withYear: true}), to: PSL.day(item.period.to, {withYear: true})})
            }),
            el('p', {
                class: 'page-sub', text: t(item.pdf === 'rendered' ? 'disc.detail.sub.rendered' : 'disc.detail.sub.kept', {
                    intent: PSL.hasText(intent) ? t(intent)
                        : t('disc.detail.intent.other', {intent: item.intent.charAt(0).toUpperCase() + item.intent.slice(1)}),
                    confidentiality: enumWord('confidentiality', item.confidentiality),
                    duration: PSL.dur(Date.now() - item.published_at_ms)
                })
            }),
            el('div', {class: 'disc-chips'}, disclosureChips(item)),
            detailActions(detail, manage),
            state.disclosureNote ? el('p', {class: 'disc-note', role: 'status', text: state.disclosureNote}) : null
        ].forEach(function (node) {
            if (node) section.appendChild(node);
        });
        if (state.withdrawOpen && manage && !item.withdrawn) section.appendChild(withdrawForm(item));
        disclosureBanners(item).forEach(function (banner) {
            section.appendChild(banner);
        });
        section.appendChild(detailPanel(t('disc.glance.head'), null, [
            disclosureFigures(item.figures),
            carbonBracket(item.figures && item.figures.carbon_kgco2eq)
        ]));
        [applicationsPanel(state.disclosureReport, item.figures), integrityPanel(detail), filesPanel(item),
            shareLinksPanel(detail, manage), deliveriesPanel(detail, manage), methodPanel(state.disclosureReport)]
            .forEach(function (panel) {
                if (panel) section.appendChild(panel);
            });
        return section;
    }

    // ---------------------------------------------------- screen: recent runs

    // --------------------------------------------------------- screen: access

    /**
     * Who the caller is and what they reach, from /api/access, reached from the identity
     * chip. Read-only but for revoking their own terminal tokens: rights live in the Hub's
     * configuration alone. `banner` is the no-grant text when /api/status refused the
     * caller, who then gets what to send an administrator instead of an empty reach.
     */
    function renderAccessScreen(banner) {
        const access = state.access;
        const signedIn = PSL.accessSignedIn(access, Date.now());
        const section = el('section', {class: 'access-screen'}, [
            ruledOverline(t('access.overline')),
            el('h1', {class: 'page-title', text: t('access.title')}),
            el('p', {class: 'page-sub', text: t('access.sub')})
        ]);
        if (banner) section.appendChild(noGrantBanner(banner));

        section.appendChild(accessCard(t('access.card.signed_in'), [
            el('p', {text: signedIn.line}),
            signedIn.note ? el('p', {class: 'access-muted', text: signedIn.note}) : null,
            el('p', {}, [
                el('span', {text: signedIn.subjectLabel}),
                el('code', {class: 'access-subject', text: signedIn.subject})
            ])
        ]));
        section.appendChild(accessCard(t('access.card.groups'), [
            access.groups.length === 0
                ? el('p', {class: 'access-muted', text: t('access.groups.none')})
                : el('ul', {class: 'access-chips', 'aria-label': t('access.groups.label')}, access.groups.map(function (group) {
                    return el('li', {
                        class: 'chip',
                        'data-origin': group.origin,
                        text: group.name + ' · ' + group.origin
                    });
                }))
        ]));
        if (banner) {
            // The banner already says nothing is reached. What helps now is the request.
            section.appendChild(terminalBlock({
                head: t('access.request.head'),
                sub: t('access.request.sub'),
                text: PSL.accessRequestText(access),
                copyLabel: t('access.request.copy')
            }));
        } else {
            const reach = PSL.accessReachRows(access);
            section.appendChild(accessCard(t('access.card.reach'), [
                reach.length === 0
                    ? el('p', {text: PSL.ACCESS_NO_REACH})
                    : accessTable(t('access.reach.caption'),
                        [t('access.col.project'), t('access.col.group'), t('access.col.role'), t('access.col.permissions')],
                        reach.map(function (row) {
                            return [row.project, row.group, row.role, row.permissions];
                        }))
            ]));
        }
        if (state.tokens || state.tokensFailed) {
            section.appendChild(tokensCard((state.tokens || []).filter(function (token) {
                return token.subject === access.subject;
            }), t('access.card.own_tokens'), 'tokens-own'));
        }
        if (access.admin) {
            section.appendChild(el('p', {class: 'access-pointer'}, [
                el('span', {text: t('access.admin.lead')}),
                el('a', {href: '#/coverage', text: t('shell.tab.coverage')}),
                el('span', {text: t('access.admin.tail')})
            ]));
        } else if (access.projects.length > 0) {
            section.appendChild(accessCard(t('access.card.projects'), [projectsTable(access.projects)]));
        }
        return section;
    }

    /**
     * The administrator's screen: what no project covers first, since that is the one list
     * to act on, then everyone's terminal tokens, then the configuration folded by part.
     */
    function renderCoverageScreen() {
        const access = state.access;
        const section = el('section', {class: 'access-screen'}, [
            ruledOverline(t('coverage.overline')),
            el('h1', {class: 'page-title', text: t('coverage.title')}),
            el('p', {class: 'page-sub', text: t('coverage.sub')})
        ]);
        if (access.unmatched) section.appendChild(attentionCard(access.unmatched));
        const snippet = access.unmatched ? PSL.coverageSnippet(access.unmatched) : null;
        if (snippet) {
            section.appendChild(terminalBlock({
                head: t('coverage.snippet.head'),
                sub: t('coverage.snippet.sub'),
                text: snippet,
                copyLabel: t('coverage.snippet.copy')
            }));
        }
        if (access.conflicts) section.appendChild(conflictsCard(access.conflicts));
        if (state.tokens || state.tokensFailed) {
            section.appendChild(tokensCard(state.tokens || [], t('coverage.card.tokens'), 'tokens-all'));
        }
        if (access.configuration) section.appendChild(configurationCard(access.configuration));
        return section;
    }

    /** A card whose title is a heading, the slashes kept for the eye and hidden from a reader. */
    function accessCard(title, children, headingId) {
        return el('section', {class: 'card access-card'}, [
            el('h2', {class: 'overline access-card-title', id: headingId, tabindex: headingId ? '-1' : null}, [
                el('span', {'aria-hidden': 'true', text: '// '}),
                el('span', {text: title})
            ])
        ].concat(children));
    }

    /** Every cell a text node. `rows` holds strings, or arrays of strings for one line each. */
    function accessTable(caption, headings, rows) {
        return el('div', {class: 'sources-wrap'}, [
            el('table', {class: 'table access-table'}, [
                el('caption', {class: 'visually-hidden', text: caption}),
                el('thead', {}, [el('tr', {}, headings.map(function (heading) {
                    return el('th', {scope: 'col', text: heading});
                }))]),
                el('tbody', {}, rows.map(function (row) {
                    return el('tr', {}, row.map(function (cell) {
                        if (cell instanceof Node) return el('td', {}, [cell]);
                        return Array.isArray(cell)
                            ? el('td', {}, cell.map(function (line) {
                                return el('div', {text: line});
                            }))
                            : el('td', {text: cell});
                    }));
                }))
            ])
        ]);
    }

    /** A source's display name, its id when the Hub no longer lists it. */
    function sourceName(id) {
        const source = (state.sources || []).find(function (candidate) {
            return candidate.id === id;
        });
        return source ? source.name : id;
    }

    /**
     * The terminal tokens `perf-sentinel login` obtained, each with a Revoke that asks once
     * more before it acts. Revoking takes effect on the token's next request, and the
     * outcome is announced in the card, which takes the focus once the list is redrawn.
     */
    function tokensCard(tokens, title, headingId) {
        const rows = PSL.tokenRows(tokens);
        const notice = state.tokensNotice && state.tokensNotice.card === headingId ? state.tokensNotice : null;
        return accessCard(title, [
            el('p', {text: t('access.tokens.intro')}),
            el('div', {role: 'status', class: 'access-status'}, notice ? [el('div', {
                class: 'banner',
                'data-tone': notice.ok ? 'ok' : 'crit'
            }, [el('div', {text: notice.text})])] : []),
            state.tokensFailed
                ? el('div', {class: 'banner', 'data-tone': 'crit'}, [
                    el('div', {text: t('access.tokens.failed')})
                ])
                : rows.length === 0
                    ? el('p', {class: 'access-muted', text: t('access.tokens.none')})
                    : accessTable(t('access.tokens.caption'), [t('access.col.owner'), t('access.col.requested_from'),
                        t('access.col.created'), t('access.col.expires'), t('access.col.last_used'), ''],
                        rows.map(function (row) {
                            return [row.owner, row.from, row.created, row.expires, row.lastUsed, revokeButton(row, headingId)];
                        }))
        ], headingId);
    }

    function revokeButton(row, card) {
        const label = t('access.revoke.label', {owner: row.owner, created: row.created});
        const button = el('button', {type: 'button', class: 'pill-button', text: t('ack.button.revoke'), 'aria-label': label});
        let armed = false;
        const disarm = function () {
            armed = false;
            button.textContent = t('ack.button.revoke');
            button.setAttribute('aria-label', label);
            button.removeAttribute('data-armed');
        };
        // Leaving the button puts it back: a second click is a decision, not a stray one.
        button.addEventListener('blur', disarm);
        button.addEventListener('keydown', function (event) {
            if (event.key === 'Escape') disarm();
        });
        button.addEventListener('click', function () {
            if (!armed) {
                armed = true;
                button.textContent = t('access.revoke.confirm');
                button.setAttribute('aria-label', t('access.revoke.confirm_label', {label: label}));
                button.setAttribute('data-armed', 'true');
                return;
            }
            button.disabled = true;
            const request = PSL.tokenRevokeRequest(row.id);
            fetch(request.path, request.init).then(signInOn401).then(function (response) {
                return response.status;
            }, function () {
                return 0;
            }).then(function (status) {
                state.tokensNotice = Object.assign({card: card}, PSL.revokeOutcome(status, row));
                return loadTokens();
            }).then(function () {
                const heading = document.getElementById(card);
                if (heading) heading.focus();
            });
        });
        return button;
    }

    function projectsTable(projects) {
        return accessTable(t('access.projects.caption'), [t('access.col.project'), t('access.col.patterns')],
            projects.map(function (project) {
                return [project.name, project.patterns.map(PSL.describePattern)];
            }));
    }

    /** One folded part of the configuration, its count in the summary so a closed fold still says something. */
    function configurationPart(title, count, body) {
        return el('details', {class: 'access-part'}, [
            el('summary', {}, [
                el('span', {class: 'access-part-title', text: title}),
                el('span', {class: 'access-part-count', text: count == null ? '' : String(count)})
            ]),
            count === 0 ? el('p', {class: 'access-muted', text: t('access.config.none')}) : body
        ]);
    }

    function configurationCard(configuration) {
        const now = Date.now();
        const part = function (name, count, headings, rows) {
            const title = t('access.config.' + name);
            return configurationPart(title, count, accessTable(title, headings.map(function (heading) {
                return t('access.col.' + heading);
            }), rows));
        };
        const none = t('access.config.nobody');
        return accessCard(t('access.config.title'), [
            el('p', {class: 'access-muted', text: t('access.config.loaded')}),
            configurationPart(t('access.config.projects'), configuration.projects.length,
                projectsTable(configuration.projects)),
            part('roles', configuration.roles.length, ['role', 'permissions'], configuration.roles.map(function (role) {
                return [role.built_in ? role.name + ' ' + t('access.config.built_in') : role.name, role.permissions.join(', ')];
            })),
            part('grants', configuration.grants.length, ['group', 'project', 'role'], configuration.grants.map(function (grant) {
                return [grant.group, grant.project, grant.role];
            })),
            part('internal_groups', configuration.internal_groups.length, ['internal_group', 'members'],
                configuration.internal_groups.map(function (group) {
                    return ['internal:' + group.name, group.members.length === 0 ? t('access.config.no_member') : group.members];
                })),
            configurationPart(t('access.config.admins'), configuration.admins.length + configuration.admin_groups.length,
                el('dl', {class: 'access-pairs'}, [
                    el('dt', {text: t('access.config.by_id')}),
                    el('dd', {text: configuration.admins.length === 0 ? none : configuration.admins.join(', ')}),
                    el('dt', {text: t('access.config.by_group')}),
                    el('dd', {text: configuration.admin_groups.length === 0 ? none : configuration.admin_groups.join(', ')})
                ])),
            part('machine_tokens', configuration.machine_tokens.length, ['machine_token', 'group', 'expires'],
                configuration.machine_tokens.map(function (token) {
                    return [token.name, token.group, PSL.accessExpiry(token.expires_at_ms, now)];
                })),
            part('local_accounts', configuration.local_accounts.length, ['local_account', 'signs_in_as'],
                configuration.local_accounts.map(function (name) {
                    return [name, 'local:' + name];
                }))
        ]);
    }

    /** What no project covers, the one part of the screen that asks for a change. */
    function attentionCard(unmatched) {
        const count = unmatched.services.length + unmatched.attributes.length + unmatched.sources.length;
        if (count === 0) {
            return accessCard(t('coverage.attention.title'), [el('p', {text: t('coverage.attention.none')})]);
        }
        const parts = [el('p', {text: t('coverage.attention.count.' + PSL.plural(count), {count: count})})];
        if (unmatched.services.length > 0) {
            parts.push(accessTable(t('coverage.services.caption'),
                [t('access.col.service'), t('access.col.seen_at'), t('access.col.findings')],
                unmatched.services.map(function (row) {
                    return [row.service == null ? t('coverage.no_service') : row.service,
                        row.source_ids.map(sourceName).join(', '), String(row.findings)];
                })));
        }
        if (unmatched.attributes.length > 0) {
            parts.push(accessTable(t('coverage.attributes.caption'), [t('access.col.attribute'), t('access.col.findings')],
                unmatched.attributes.map(function (row) {
                    return [row.key + '=' + row.value, String(row.findings)];
                })));
        }
        if (unmatched.sources.length > 0) {
            parts.push(accessTable(t('coverage.sources.caption'), [t('access.col.unnamed_source'), t('access.col.findings')],
                unmatched.sources.map(function (row) {
                    // A trace backend stores no finding of its own: its runs are read live.
                    return [sourceName(row.source_id), row.findings == null ? t('coverage.backend') : String(row.findings)];
                })));
        }
        if (unmatched.truncated) {
            parts.push(el('p', {class: 'access-muted', text: t('coverage.truncated')}));
        }
        return accessCard(t('coverage.attention.title'), parts);
    }

    function conflictsCard(conflicts) {
        return accessCard(t('coverage.conflicts.title'), [
            el('p', {text: t('coverage.conflicts.body')}),
            conflicts.copies.length === 0
                ? el('p', {class: 'access-muted', text: t('coverage.conflicts.none')})
                : accessTable(t('coverage.conflicts.caption'),
                    ['source', 'signature', 'service', 'key', 'values', 'missing_key'].map(function (heading) {
                        return t('access.col.' + heading);
                    }),
                    conflicts.copies.map(function (row) {
                        return [sourceName(row.source_id), row.signature, row.service || t('coverage.no_service'), row.key,
                            row.values.join(', '), String(row.identities_without_key)];
                    })),
            conflicts.truncated ? el('p', {class: 'access-muted', text: t('coverage.conflicts.truncated')}) : null
        ]);
    }

    function renderRecentScreen() {
        const section = el('section', {}, [
            ruledOverline(t('recent.overline')),
            el('h1', {class: 'page-title', text: t('recent.title')}),
            el('p', {class: 'page-sub', text: t('recent.sub', {hours: state.status.limits.report_retention_hours})})
        ]);

        if (!state.runs) {
            section.appendChild(el('div', {class: 'card skeleton', style: 'height:120px;margin-top:18px'}));
            return section;
        }
        if (state.runs.length === 0) {
            section.appendChild(el('div', {class: 'empty-state'}, [
                el('p', {
                    class: 'empty-title', text: t('recent.empty.title', {hours: state.status.limits.report_retention_hours})
                }),
                el('p', {text: t('recent.empty.body', {hours: state.status.limits.report_retention_hours})}),
                el('div', {class: 'outcome-actions empty-actions'}, [
                    el('a', {class: 'action action-filled', href: '#/new', text: t('recent.run_new')})
                ])
            ]));
            return section;
        }

        const binaries = Array.from(new Set(
            state.runs.map(function (run) {
                return run.producer_version;
            }).filter(Boolean))).sort(PSL.vcmp);
        if (binaries.length > 1) {
            section.appendChild(el('div', {class: 'banner', 'data-tone': 'warn'}, [
                svg([['path', {d: 'M10.3 3.9 1.9 18a2 2 0 0 0 1.7 3h16.8a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z'}],
                    ['path', {d: 'M12 9v4M12 17h.01'}]], 16),
                el('p', {text: t('recent.binaries', {versions: binaries.join(t('recent.and')), count: binaries.length})})
            ]));
        }

        const tuned = state.runs.filter(function (run) {
            return Object.keys((run.request || {}).detection || {}).length > 0;
        });
        if (tuned.length > 0 && tuned.length < state.runs.length) {
            section.appendChild(el('div', {class: 'banner', 'data-tone': 'warn'}, [
                warningGlyph(16),
                el('p', {text: t('recent.tuned.' + PSL.plural(tuned.length), {count: tuned.length})})
            ]));
        }

        section.appendChild(el('div', {class: 'run-list'}, state.runs.map(runCard)));
        return section;
    }

    function runCard(run) {
        const key = PSL.statusKey(run);
        const card = el('a', {class: 'run-card', 'data-status': key, href: '#/run/' + run.id});

        card.appendChild(el('span', {class: 'run-card-line'}, [
            el('span', {class: 'status-pill', 'data-status': key, text: t(key === 'empty' ? 'recent.status.empty' : 'status.' + key)}),
            el('span', {class: 'run-card-name', text: run.source_name}),
            el('span', {class: 'chip', text: PSL.KIND_LABEL[run.kind] || run.kind}),
            el('span', {
                class: 'chip chip-declared',
                text: run.environment,
                title: t('recent.env_title')
            }),
            el('span', {class: 'run-card-spacer'}),
            el('span', {class: 'run-card-id', text: run.id})
        ]));
        card.appendChild(el('span', {class: 'run-card-args', text: PSL.argsLine(run), title: PSL.argsLine(run)}));
        card.appendChild(el('span', {class: 'run-card-facts'}, cardFacts(run, key).map(function (fact) {
            return el('span', {class: 'fact'}, [
                el('span', {class: 'fact-k', text: fact[0]}),
                live(el('span', {class: 'fact-v', 'data-tone': fact[2] || 'mono', text: fact[1]}), fact[3])
            ]);
        })));
        return card;
    }

    /** Durations relative to now, the way an operator reads a list: "3 s", not a clock. */
    function cardFacts(run, key) {
        const now = Date.now();
        const started = run.started_at_ms || run.created_at_ms;
        const ran = run.finished_at_ms
            ? PSL.dur(run.finished_at_ms - started)
            : key === 'queued' ? t('recent.not_started') : t('recent.so_far', {duration: PSL.dur(now - started)});
        const facts = [[t('recent.fact.by'), run.requested_by], [t('recent.fact.ran'), ran]];
        if (run.producer_version) facts.push([t('run.fact.' + PSL.detector(run.kind)), run.producer_version,
            PSL.skew(run.producer_version) ? 'warn' : 'mono']);
        facts.push([t('recent.fact.started'), t('sources.ago', {duration: PSL.dur(now - started)})]);
        facts.push([t('recent.fact.expires'), run.expires_at_ms ? expiryText(run) : t('unit.na'),
            run.expires_at_ms && run.expires_at_ms < now ? 'crit' : 'mono',
            run.expires_at_ms ? function () {
                return expiryText(run);
            } : null]);
        const tuned = Object.keys((run.request || {}).detection || {});
        if (tuned.length > 0) {
            facts.push([t('recent.fact.thresholds'), t('recent.changed.' + PSL.plural(tuned.length), {count: tuned.length}),
                'warn']);
        }
        if (run.error_code) facts.push([t('recent.fact.error'), run.error_code, 'crit']);
        return facts;
    }

    function loadRuns() {
        // limit=500 rather than the API's 50 default: the weight history
        // filters per source, and on a busy multi-source Hub the newest 50
        // can all belong to someone else.
        return getJson('/api/analyses?limit=500').then(function (runs) {
            state.runs = runs;
            applyRuns();
        }).catch(function () {
            state.runs = [];
            applyRuns();
        });
    }

    /**
     * On the launcher, refresh the weight-history block in place instead of
     * re-rendering: the deferred fetch would otherwise replace the whole
     * form mid-interaction and drop focus, caret and slider capture, the
     * same hazard the slider handler documents.
     */
    function applyRuns() {
        if (currentScreen() !== 'new') {
            render();
            return;
        }
        const slot = document.getElementById('weight-history');
        if (!slot) return;
        slot.replaceChildren();
        const history = weightHistory();
        if (history) slot.appendChild(history);
    }

    // ------------------------------------------------- screen: no such address

    function renderMissingScreen() {
        const route = (location.hash || '').replace('#/', '').split('?')[0];
        const access = route === 'access' || route === 'coverage';
        return el('section', {class: 'gone'}, [
            el('h1', {
                class: 'page-title',
                text: t(access ? 'missing.access.title' : 'missing.title')
            }),
            el('p', {
                class: 'page-sub',
                text: access ? t('missing.access.body') : t('missing.body', {route: route})
            }),
            el('div', {class: 'outcome-actions'}, [
                el('a', {class: 'action action-filled', href: '#/new', text: t('recent.run_new')}),
                el('a', {class: 'action action-secondary', href: '#/recent', text: t('run.all')})
            ])
        ]);
    }

    // ------------------------------------------------ screen: dashboard handoff

    /**
     * The report is served byte for byte as the engine produced it, in a frame of
     * its own. The surface changes visibly so the operator knows they left the
     * launcher, and the single return is always present.
     */
    function renderReportScreen(id) {
        const run = state.run && state.run.id === id ? state.run : null;
        const key = run ? PSL.statusKey(run) : null;
        const stage = el('div', {class: 'report-stage'});
        const shell = el('div', {class: 'report-shell'}, [reportBar(id, run), stage]);
        if (state.runError === id) {
            stage.appendChild(el('div', {class: 'gone report-gone'}, [unknownRun()]));
            return shell;
        }
        // A report runs to megabytes: until the frame loads it would read as a blank page.
        const loading = el('div', {class: 'report-loading', role: 'status'}, [
            el('span', {class: 'report-loading-band', 'aria-hidden': 'true'}),
            el('span', {class: 'report-loading-text', text: t('report.loading')})
        ]);
        if (!run) {
            stage.appendChild(loading);
            return shell;
        }
        if (key !== 'succeeded' && key !== 'empty') {
            stage.appendChild(reportGone(run, key));
            return shell;
        }
        const frame = el('iframe', {class: 'report-frame', src: '/reports/' + id + '.html', title: t('report.frame_title')});
        frame.addEventListener('load', function () {
            loading.remove();
            // A report deleted under a run that still reads succeeded comes back as
            // an empty 404, which the frame would show as a blank page.
            const body = frame.contentDocument && frame.contentDocument.body;
            if (body && !body.firstElementChild && body.textContent.trim() === '') {
                stage.replaceChildren(reportGone(run, 'expired'));
            }
        });
        stage.append(frame, loading);
        return shell;
    }

    function reportBar(id, run) {
        const lifetime = live(el('span', {class: 'report-engine', text: reportLifetime(id)}),
            function () {
                return reportLifetime(id);
            });
        // Without a run, the page under the bar carries the ways out, so the bar does not repeat one.
        return el('div', {class: 'report-bar'}, [
            run ? el('a', {class: 'pill-button', href: '#/run/' + id}, [
                svg([['path', {d: 'M14 6l-6 6 6 6'}]], 14),
                el('span', {text: t('report.back')})
            ]) : null,
            el('span', {class: 'report-path', text: t('report.path', {id: id})}),
            el('span', {class: 'report-spacer'}),
            lifetime
        ]);
    }

    /** Why a report link opens on nothing, and the way on from there. */
    function reportGone(run, key) {
        const view = ['queued', 'running', 'failed', 'interrupted', 'expired'].indexOf(key) >= 0 ? key : 'other';
        const finished = key !== 'queued' && key !== 'running';
        return el('div', {class: 'gone report-gone'}, [
            el('h1', {class: 'page-title', text: t('report.gone.' + view + '.title')}),
            el('p', {class: 'page-sub', text: t('report.gone.' + view + '.body', {name: run.source_name})}),
            el('div', {class: 'outcome-actions'}, [
                finished ? actionButton(rerunAction(t('report.rerun'), true), true) : null,
                el('a', {class: 'action action-secondary', href: '#/run/' + run.id, text: t('report.open_run')})
            ]),
            finished ? el('p', {class: 'outcome-note', text: PSL.argsLine(run)}) : null
        ]);
    }

    function reportLifetime(id) {
        const run = state.run && state.run.id === id ? state.run : null;
        const version = state.status && state.status.engine_version;
        // No run, no report: crediting an engine with it would describe a page that does not exist.
        if (!run) return '';
        const rendered = t('report.rendered', {version: version || t('shell.version.unknown')});
        if (!run.expires_at_ms) return rendered;
        const left = run.expires_at_ms - Date.now();
        return rendered + (left > 0 ? t('report.expires_in', {duration: PSL.durPrecise(left)}) : t('report.expired'));
    }

    // ------------------------------------------------------------------ boot

    // A Grafana link arrives as `/?ack=<signature>`, with the environment and the
    // source it was opened from when it has them. It becomes the hash route
    // before anything reads the route, and the query leaves the address bar.
    const ackEntry = PSL.ackEntryHash(location.search);
    if (ackEntry) history.replaceState(null, '', location.pathname + ackEntry);
    // Before the theme: its label is read from the tables in the shown language.
    initLang();
    initTheme();
    // Before the first render, so a row left open comes back open and reads its
    // daemon on its own rather than waiting to be clicked again.
    restoreFolds();
    restoreShell();
    // Escape closes the picker. Without it the only ways out are Apply, a quick
    // range or a click outside, and a keyboard user has none of them.
    globalThis.addEventListener('keydown', function (event) {
        if (event.key !== 'Escape') return;
        // A tip must be dismissable without moving the pointer or the focus (WCAG 1.4.13).
        if (openTip) {
            closeTip();
            return;
        }
        if (state.form.pickerOpen) closePicker();
    });
    render();
    loadShell();
    globalThis.addEventListener('hashchange', onRoute);
})();
