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
    const THEME_LABELS = {auto: 'System', light: 'Light', dark: 'Dark'};

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
        document.getElementById('theme-label').textContent = THEME_LABELS[state.themePosition];
        button.setAttribute('aria-label', 'Theme: ' + THEME_LABELS[state.themePosition] + '. Click to cycle.');
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
                const error = new Error(path + ' answered ' + response.status);
                error.status = response.status;
                throw error;
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
        state.daemonGroupOpen = PSL.openFolds(stored.group);
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
        document.getElementById('version-hub').textContent = status ? status.version : 'unknown';
        document.getElementById('version-engine').textContent =
            status && status.engine_version ? status.engine_version : 'none';
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
            identity.title = 'Your access: who you are, your groups and what you reach.';
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
            ['hub', PSL.updateState(state.status.version, state.status.latest_hub_version), PSL.hubReleaseUrl()],
            ['engine', PSL.updateState(state.status.engine_version, state.status.latest_engine_version), null]
        ].filter(function (row) {
            return row[1];
        });
        if (behind.length === 0) return;

        const segment = el('span', {
            class: 'shell-version-update',
            title: 'The newest release published on GitHub, read by this Hub on its update-check '
                + 'interval. It says a newer version exists, not that this one is wrong.'
        }, [
            svg([['path', {d: 'M12 19V5M5 12l7-7 7 7'}]], 12),
            // Said, not left to be inferred from two numbers side by side. The Hub
            // asked and got an answer, so it can claim this much.
            el('span', {class: 'shell-version-news', text: 'update available'})
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
            el('span', {text: 'fleet ' + oldest + ' → ' + state.status.engine_version})
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
            'aria-label': topic ? 'About ' + topic.replace(/^\/\/\s*/, '') : 'Help'
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
        proseInto(parent, version
            ? 'Install the `perf-sentinel` binary on the machine you will type this into. This Hub '
            + 'runs ' + version + ' and the command is spelled for it, an older engine may refuse a '
            + 'flag it does not have yet. Take the same build: '
            : 'Install the `perf-sentinel` binary on the machine you will type this into. This Hub '
            + 'reports no engine version, so there is none to match here: ');
        parent.appendChild(el('a', {
            class: 'terminal-link',
            href: PSL.releaseUrl(version),
            // Someone else's site in someone else's tab: the Hub keeps the page the
            // reader was working on, and hands the opener nothing.
            target: '_blank',
            rel: 'noopener noreferrer',
            text: version ? 'perf-sentinel ' + version + ' on GitHub' : 'the perf-sentinel releases'
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
        proseInto(step, 'This source is behind an `' + source.auth_header_name + '` header. Run this '
            + 'in the same terminal, before the command, with your own credential where the dots are:');
        const line = el('code', {
            class: 'code-inline step-line',
            text: PSL.exportLine(state.shell, 'PERF_SENTINEL_SOURCE_TOKEN', header)
        });
        SPELL.set(line, function (shellId) {
            return PSL.exportLine(shellId, 'PERF_SENTINEL_SOURCE_TOKEN', header);
        });
        line.setAttribute('data-spell', '');
        step.appendChild(line);
        proseInto(step, 'The whole header line goes in, name included, not the value on its own. The '
            + 'Hub holds a header of its own for this source and never discloses it, which is why the '
            + 'command reads yours from a variable rather than carrying a secret where `ps` would '
            + 'show it to everyone on the machine.');
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
                el('h2', {class: 'overline', text: '// what you need first'})
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

        const strip = el('div', {class: 'cmd-tabs', role: 'tablist', 'aria-label': 'Shell'}, tabs);
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
        // What this block holds, for the sentences below: the same scaffold renders
        // a command and a configuration file, and neither noun fits both.
        const subject = spec.download ? 'file' : 'command';
        const label = el('span', {text: 'Copy'});
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
                label.textContent = copied ? 'Copied' : 'Copy';
                if (copied) button.setAttribute('data-copied', 'true');
                status.textContent = copied
                    ? 'Copied.'
                    : 'This browser refused the copy. The ' + subject
                    + ' is selected, use your own copy key.';
                clearTimeout(timer);
                timer = setTimeout(function () {
                    label.textContent = 'Copy';
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
            'aria-label': 'Download ' + filename
        }, [downloadGlyph(), el('span', {text: 'Download'})]);
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
            status.textContent = 'Sent ' + filename + ' to your browser. It decides where it lands, '
                + 'and may rename a file whose name starts with a dot.';
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
        } else if (screen === 'disclosures') loadDisclosures();
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
            ruledOverline('// sources'),
            el('h1', {class: 'page-title', text: 'Fleet health'}),
            el('p', {
                class: 'page-sub',
                text: 'Every column is observed except the environment, which each source declares about itself. '
                    + 'Sources are configured at deploy time, not from here.'
            })
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
                el('div', {
                    text: 'The Hub is not answering, so fleet health is unknown. This is the Hub itself, '
                        + 'not any one source. Nothing below is shown rather than showing values that may be stale.'
                })
            ]));
            return section;
        }

        section.appendChild(el('div', {class: 'sources-wrap'}, [sourcesTable(state.sources)]));
        return section;
    }

    const SOURCE_COLUMNS = [
        'Source', 'Type', 'Env (declared)', 'Health', 'Last success', 'Unreachable for', 'Producer', 'Last error'
    ];
    // The columns whose cells are right-aligned. The headings have to follow, or
    // a value sits under the gap beside its own label.
    const SOURCE_COLUMNS_RIGHT = ['Last success', 'Unreachable for', 'Producer'];

    function sourcesTable(sources) {
        const head = el('tr', {}, SOURCE_COLUMNS.map(function (name) {
            return el('th', {
                text: name,
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
            ? [tableGroupRow('daemons')].concat(rows(kinds.daemons),
                [tableGroupRow('trace backends')],
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
                el('span', {class: 'source-locked', text: 'details need sources.read'})
            ]));
        } else row.appendChild(el('td', {class: 'table-strong', text: source.name}));
        row.appendChild(el('td', {}, [el('span', {class: 'chip', text: PSL.KIND_LABEL[source.kind] || source.kind})]));
        row.appendChild(el('td', {}, [el('span', {class: 'chip chip-declared', text: source.environment})]));
        row.appendChild(el('td', {}, [healthCell(source, now)]));
        // Ages, so they tick with the panel beside them instead of freezing at the render.
        const lastSuccess = function () {
            return source.last_success_ms ? PSL.dur(Date.now() - source.last_success_ms) + ' ago' : 'never';
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
                    text: 'unknown',
                    title: 'This daemon reports a producer version, but the Hub has not had a successful '
                        + 'response from it yet.'
                })
                : el('td', {
                    class: 'table-muted',
                    'data-align': 'right',
                    text: 'n/a',
                    title: 'A trace backend stores traces and detects nothing, so it reports no producer version.'
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
                title: 'perf-sentinel is pre-1.0, so detectors change between minors. The Hub compares two '
                    + 'version strings and cannot know whether this minor changed detection.'
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
        ['// declared', 'Not measured. This is what the daemon says it is.', ['environment']],
        ['// ingestion and memory', 'What it takes in, and what it drops when it cannot keep up.',
            ['sampling_rate', 'max_active_traces', 'trace_ttl_ms', 'max_events_per_trace',
                'max_payload_size', 'ingest_queue_capacity', 'analysis_queue_capacity',
                'memory_high_water_pct']],
        ['// what it keeps for readers',
            'Ring buffers behind the query API and the export. Past each one, the oldest goes.',
            ['max_retained_findings', 'max_export_findings', 'max_retained_traces']],
        ['// metrics labels',
            'How /metrics splits its series. Emptying one folds its label away, which is breaking '
            + 'for whatever already reads it.',
            ['per_service_labels', 'per_grouping_labels']],
        ['// listeners', 'Where it accepts spans, and whether it answers questions at all.',
            ['api_enabled', 'listen_addr', 'listen_port', 'listen_port_grpc', 'json_socket']],
        ['// sub-systems', 'Off unless somebody turned them on.',
            ['tls_configured', 'ack_enabled', 'ack_api_key_set', 'cors_allowed_origins',
                'archive_configured']],
        ['// correlation',
            'Off by default. Every field under the first one applies only while it is on.',
            ['correlation_enabled', 'correlation_window_ms', 'correlation_lag_threshold_ms',
                'correlation_min_co_occurrences', 'correlation_min_confidence',
                'correlation_max_tracked_pairs']]
    ];

    /** What each setting costs when it is wrong. Two say enough by their name. */
    const DAEMON_COPY = {
        environment: 'The label this daemon stamps on every finding, which sets their confidence: '
            + 'staging reads as medium, production as high. Declared, like the column on the row above.',
        sampling_rate: 'The share of arriving traces it analyses. Below 100 % every aggregate in a '
            + 'snapshot is a sample of the traffic, not the traffic.',
        max_active_traces: 'How many traces it correlates in memory at once. The oldest is evicted '
            + 'past this, and a trace evicted while spans are still arriving is analysed incomplete.',
        trace_ttl_ms: 'How long a trace waits for more spans before it is closed and analysed.',
        max_events_per_trace: 'The ring buffer inside one trace. Its oldest spans drop once it fills, '
            + 'and the finding says nothing about what left.',
        max_payload_size: 'The largest single request body it will deserialize. Anything larger is '
            + 'refused whole, never truncated.',
        ingest_queue_capacity: 'Span batches buffered between the listeners and the event loop. A '
            + 'full queue pushes back on the sender as an OTLP 503.',
        analysis_queue_capacity: 'Batches waiting for detection. A full queue sheds whole batches, '
            + 'and a shed batch is silent to whoever sent it.',
        memory_high_water_pct: 'The share of memory above which it refuses new spans rather than meet '
            + 'the OOM killer. Zero disables the guard entirely.',
        max_retained_findings: 'Findings held for the query API. The oldest are evicted past this, so '
            + 'an old problem can leave a snapshot without anyone fixing it.',
        max_export_findings: 'Findings one export snapshot carries. The quality gate inside that '
            + 'snapshot counts those and no others.',
        max_retained_traces: 'Span trees kept so an export can draw them. Zero keeps none, and every '
            + 'finding in that export opens without a tree.',
        per_service_labels: 'Whether the finding, slow-duration and I/O series carry the service '
            + 'they came from. Off empties the label, and every service folds into one series.',
        per_grouping_labels: 'The same for the grouping beside it, the namespace by default. Off '
            + 'empties that label, and past the daemon\'s own cap on (service, grouping) pairs a '
            + 'grouping folds into `_other` on its own.',
        api_enabled: 'Whether the query API is served at all. The Hub reads this daemon through it, '
            + 'so a run from here needs it on.',
        listen_addr: 'Where the OTLP receivers and /metrics bind. An address outside loopback exposes '
            + 'both without authentication.',
        json_socket: 'Unix socket for native NDJSON ingestion, alongside OTLP.',
        tls_configured: 'TLS on the OTLP listeners. The Hub is told whether a certificate and key are '
            + 'set, never where they are.',
        ack_enabled: 'The daemon\'s own acknowledgement store. An acknowledged finding stays in the '
            + 'data and stops counting against the gate.',
        ack_api_key_set: 'Whether the acknowledgement routes require a key. The Hub is told that one '
            + 'exists, never what it is.',
        cors_allowed_origins: 'Browser origins the query API answers. Empty sends no CORS headers at all.',
        archive_configured: 'Whether it writes a report archive per window, for a later disclosure to '
            + 'read back.',
        correlation_enabled: 'Whether the cross-trace correlator runs.',
        correlation_window_ms: 'The rolling window over which two findings count as having happened '
            + 'together.',
        correlation_lag_threshold_ms: 'The largest gap between two findings that still counts as together.',
        correlation_min_co_occurrences: 'How many times a pair has to happen before it is reported at all.',
        correlation_min_confidence: 'How often the second finding follows the first, as a share of the '
            + 'first\'s own occurrences, before the pair is worth reporting.',
        correlation_max_tracked_pairs: 'Cap on tracked pairs. The least frequent are evicted past it, '
            + 'and the daemon says so above when that happens.',
        energy_source: 'Where the energy figure comes from: the power backends that measured each '
            + 'covered service, and how many services were modeled from I/O counts instead. Calibrated '
            + 'means calibration factors rescaled that model. Database or broker hosts the daemon counts '
            + 'inside its totals follow, each with its own source.',
        api_version: 'The Electricity Maps API version these figures were scored against.',
        emission_factor_type: 'Lifecycle counts the whole chain behind the electricity, direct counts '
            + 'only what the generation itself emits.',
        temporal_granularity: 'How finely grid intensity is resolved in time.',
        electricity_maps: 'Whether Electricity Maps is configured for live grid intensity. Off means each '
            + 'region took its grid operator\'s feed when one is configured, or the embedded table, which '
            + 'is a vintage rather than a reading.',
        grid_feeds: 'The grid operator feeds configured, each giving its regions a live intensity drawn '
            + 'from its operator\'s data. They count direct emissions only, so they read lower than a '
            + 'lifecycle figure, and Electricity Maps wins for a region both cover.',
        fingrid_dataset: 'The Fingrid dataset scored. 266 counts Finnish production, 265 consumption with '
            + 'imports included, and the two bases give different figures.',
        per_operation_coefficients: 'Whether each operation kind carries its own energy coefficient '
            + 'instead of one average across all I/O.',
        use_hourly_profiles: 'Whether the hour-by-hour shape of the grid is applied rather than a '
            + 'flat average.',
        embodied_per_request_gco2: 'Embodied carbon charged per request, the manufacture share of '
            + 'the figures rather than the electricity.',
        network_energy_per_byte_kwh: 'A coefficient the engine deprecated and no longer applies, '
            + 'published for configurations that still set it.'
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
        return DAEMON_COPY[name] || DETECTION_COPY[DETECT_ALIAS[name] || name] || null;
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
                el('p', {class: 'daemon-loading', role: 'status', text: 'Reading ' + source.name + '.'}),
                el('div', {class: 'skeleton', style: 'height:150px'})
            ]);
        }
        if (view.error_code) return daemonError(source, index, view.error_code);

        return el('div', {class: 'daemon-panel'}, [
            el('p', {class: 'overline daemon-audience', text: '// intended for devops'}),
            el('p', {
                class: 'daemon-source-note',
                text: 'Reported by this daemon over its query API. The Hub relays it and verifies none of '
                    + 'it. Everything here is read-only, and every setting below is changed where the daemon '
                    + 'is deployed, in its Helm values or its own configuration file, never from this Hub.'
            }),
            el('div', {id: 'daemon-top-' + index}, [daemonTopRow(source, view, index)]),
            terminalBlock({
                head: '// the same view in your terminal',
                sub: 'The same figures, plus the tabs this screen leaves out.',
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
                copyLabel: 'Copy the monitor command for ' + source.name,
                notes: [
                    '`query monitor` carries the energy and carbon breakdown this screen only summarises. '
                    + 'It re-reads on the interval chosen above, and this line changes with it, so the '
                    + 'terminal and this row never disagree about how often the daemon is asked. Set that '
                    + 'to off and the command drops `--refresh`, leaving the engine its own default of '
                    + 'five seconds.',
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
                        el('span', {text: 'Reading this daemon\'s settings returned '}),
                        el('span', {class: 'code-inline', text: code}),
                        proseInto(el('span'),
                            ': ' + (PSL.READ_ERRORS[code] || PSL.ERRORS[code] || 'the Hub could not reach it.'))
                    ]),
                    el('p', {
                        class: 'notice-sub',
                        text: 'The row above still shows the last collection state, which is a different '
                            + 'observation made at a different time.'
                    })
                ])
            ]),
            el('p', {
                class: 'daemon-lead',
                text: 'This row asks again on its own, and shows the daemon as soon as it answers.'
            }),
            refreshControl(source, index)
        ]);
    }

    const DAEMON_VERDICT = {
        ok: ['nominal', 'ok'],
        near_capacity: ['near capacity', 'warn'],
        advised: ['advised', 'warn'],
        unknown: ['not measurable', 'muted']
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
        const verdict = DAEMON_VERDICT[view.state] || DAEMON_VERDICT.unknown;
        // Read once and cleared: the badges belong to the read that produced them,
        // not to every rebuild that happens to come after it.
        const moves = state.daemonMoves[source.id] || {};
        delete state.daemonMoves[source.id];
        const main = el('div', {class: 'daemon-top-main'}, [
            el('div', {class: 'sink-head'}, [
                titledOverline('// right now', 'Read from the daemon on the interval below. A tick asks '
                    + 'the daemon for its status alone, and once a minute the full export runs to refresh '
                    + 'the hints, so the interval prices a small read, not the heavy one.'),
                el('span', {class: 'sink-sub refresh-read', id: 'refresh-read-' + index})
            ]),
            refreshControl(source, index),
            countStrip([
                [gaugeText(view.traces), 'active traces', PSL.gaugeTone(pct(view.traces)), moves.traces],
                [gaugeText(view.analysis_queue), 'analysis queue',
                    PSL.gaugeTone(pct(view.analysis_queue)), moves.analysis_queue],
                // No tone: the ring fills on any daemon that runs long enough, and
                // the Hub keeps what it evicts. A poll that missed some says so in the hints.
                [gaugeText(view.findings), 'findings stored', null, moves.findings],
                // No cap and only one direction: an uptime that grows every read is
                // not news, and a tone would say it is running out of something. Down to
                // the minute, because two units hide a whole day: a daemon up for 10 d
                // 23 h reads the same as one up for 10 d flat.
                [view.uptime_seconds == null ? 'unknown' : PSL.durMinutes(view.uptime_seconds * 1000), 'uptime']
            ]),
            el('p', {
                class: 'daemon-lead',
                text: 'Each figure is shown against the cap it runs into. Being near one is not a problem '
                    + 'by itself, and this screen does not decide that it is: the daemon does, from counters '
                    + 'the Hub cannot see.'
            })
        ]);

        const side = el('div', {class: 'daemon-top-side'}, [
            el('div', {class: 'sink-head'}, [
                titledOverline('// what the daemon recommends', 'These come from counters inside the '
                    + 'daemon that no report and no dashboard carries. The Hub relays the sentences and '
                    + 'writes none of its own.'),
                el('span', {class: 'sink-sub', text: 'Written by the daemon, not by the Hub.'})
            ])
        ]);
        if (view.hints_unavailable_reason) {
            // An unread export is not a clean bill: silence has to be earned.
            side.appendChild(proseInto(el('p', {class: 'daemon-lead'}),
                'The export this screen reads hints from could not be read: `'
                + view.hints_unavailable_reason + '`. Whatever the daemon recommends right now is '
                + 'unknown, which is not the same thing as nothing.'));
        } else if (view.warnings.length === 0) {
            side.appendChild(el('p', {
                class: 'daemon-lead',
                text: 'Nothing. The daemon emits a hint when its own counters show a setting is undersized '
                    + 'for the load it is taking. Silence here means those counters were clean at the '
                    + 'instant it was read, not that the settings are right.'
            }));
        } else {
            view.warnings.forEach(function (hint) {
                side.appendChild(el('div', {class: 'outcome-warning'}, [
                    el('span', {
                        class: 'outcome-warning-kind',
                        'data-tone': DAEMON_HINT_TONE[hint.kind] || 'muted',
                        text: hint.kind
                    }),
                    proseInto(el('span', {class: 'outcome-warning-message'}), hint.message)
                ]));
            });
            if (view.warnings_dropped > 0) {
                side.appendChild(el('p', {
                    class: 'daemon-lead',
                    text: view.warnings_dropped + ' more arrived than the Hub relays in one view.'
                }));
            }
        }

        return el('div', {}, [
            el('div', {class: 'daemon-verdict-row'}, [
                el('span', {class: 'daemon-verdict', 'data-tone': verdict[1], text: verdict[0]})
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
    const REFRESH_CHOICES = REFRESH_SECONDS.map(function (seconds) {
        return [seconds * 1000, seconds ? seconds + ' s' : 'off'];
    });
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
        const select = el('select', {class: 'refresh-select', 'aria-label': 'Re-read interval'});
        REFRESH_CHOICES.forEach(function (choice) {
            const option = el('option', {value: String(choice[0]), text: choice[1]});
            if (choice[0] === ms) option.selected = true;
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
            el('span', {class: 'refresh-label', text: 'every'}),
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
            read.textContent = 'Read ' + PSL.dur(Date.now() - readAt) + ' ago.';
        }

        const ms = refreshMs(source.id);
        // The sweep runs on its own clock, set by startTicker and refreshDaemon.
        // Timing it from the last read would hold the disc at full for however
        // long the request takes, which reads as a stall rather than as a cycle.
        const cycleAt = state.daemonCycleAt[source.id];
        const ring = next.parentNode.querySelector('.refresh-ring');
        const fill = ring && ring.querySelector('.refresh-ring-fill');
        if (!ms) {
            next.textContent = 'Not re-reading.';
            if (ring) ring.hidden = true;
            return;
        }
        if (ring) ring.hidden = false;
        const left = Math.max(0, cycleAt + ms - Date.now());
        next.textContent = 'Next in ' + Math.ceil(left / 1000) + ' s.';
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
        if (!gauge || gauge.value == null) return 'unknown';
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
        }, [el('span', {
            text: count > 0
                ? 'The ' + count + ' applied settings, and what this daemon changed'
                : 'What this daemon detects with'
        })]);
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
            reason === 'api_disabled'
                ? 'This daemon does not serve its configuration: `api_enabled` is off in its own '
                + '`[daemon]` section. Everything above came from the export instead.'
                : reason === 'unreadable'
                    ? 'This daemon answered for its configuration with something the Hub could not '
                    + 'relay, an error status or a body that is not the `[daemon]` object. The gauges '
                    + 'above are the same daemon answering fine.'
                    : 'This daemon did not answer for its configuration, so none is shown rather than a '
                    + 'copy from some earlier moment.');
    }

    function settingsCards(sourceId, view) {
        const cards = [];
        if (view.config) {
            DAEMON_GROUPS.forEach(function (spec) {
                const present = spec[2].filter(function (name) {
                    return view.config[name] !== undefined;
                });
                if (present.length > 0) {
                    cards.push(settingsCard(
                        sourceId, spec[0], spec[1], present, view.config, view.daemon_defaults));
                }
            });
        }
        if (view.detection_config) {
            cards.push(settingsCard(
                sourceId,
                '// detection thresholds',
                'What counts as a problem. The same knobs the launcher lets a run override on a backend.',
                Object.keys(view.detection_config),
                view.detection_config,
                view.detection_defaults));
        }
        if (view.scoring_config || view.energy_source) {
            const scoring = Object.assign({}, view.scoring_config || {});
            if (view.energy_source) scoring.energy_source = view.energy_source;
            cards.push(settingsCard(
                sourceId,
                '// carbon scoring',
                'Where the energy figures come from, not what they are.',
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
            'A value this daemon changed is marked, with the engine\'s default beside it. Compared '
            + 'against perf-sentinel `' + view.defaults_engine_version + '`, the binary this Hub '
            + 'embeds. It covers the `[daemon]` and `[detection]` sections and the scoring half of '
            + '`[green]`. The gate thresholds under `[thresholds]` are not published as a section, so '
            + 'a value set there is real and simply not visible here.');
        if (view.version && view.version !== view.defaults_engine_version) {
            proseInto(
                note.appendChild(el('span', {class: 'settings-preamble-skew'})),
                ' This daemon runs `' + view.version + '`, so a default could have moved between the two '
                + 'and a value marked as changed may only be a different default.');
        }
        return note;
    }

    /**
     * One group, folded. Folded is the useful state: nine headings fit on a
     * glance, and the count of departures on each says which one to open. The
     * gloss lives in the body rather than the heading, or the folded list would
     * be as long as the open one.
     */
    function settingsCard(sourceId, head, sub, names, config, defaults) {
        const key = sourceId + '|' + head;
        const open = state.daemonGroupOpen[key] === true;
        const changed = names.filter(function (name) {
            return isChanged(name, config[name], defaults);
        }).length;

        const heading = el('span', {class: 'overline', text: head});
        const button = el('button', {
            type: 'button',
            class: 'settings-card-head',
            'aria-expanded': open ? 'true' : 'false'
        }, [
            heading,
            el('span', {class: 'settings-card-n', text: String(names.length)}),
            changed > 0
                ? el('span', {class: 'settings-card-changed', text: changed + ' changed'})
                : null
        ]);

        const body = el('div', {class: 'settings-card-body'}, [
            el('p', {class: 'sink-sub settings-card-sub', text: sub})
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
                text: 'default ' + daemonValue(name, fallback)
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
        if (value === null) return '(not set)';
        if (name === 'sampling_rate' || name === 'correlation_min_confidence') return share(value);
        // Zero is not a percentage here, it switches the guard off entirely.
        if (name === 'memory_high_water_pct') return value === 0 ? 'off' : value + ' %';
        if (name === 'max_payload_size') return PSL.bytes(value);
        if (Array.isArray(value)) return value.length === 0 ? '(none)' : value.join(', ');
        if (name === 'tls_configured' || name === 'archive_configured') {
            return value ? 'configured' : 'not configured';
        }
        if (name === 'ack_api_key_set') return value ? 'set' : 'unset';
        // The name says _ms, so the millisecond figure stays primary: it is the one
        // that goes back into the file. PSL.dur rounds to the second, so the
        // readable form only appears once there is a second to read.
        if (/_ms$/.test(name) && typeof value === 'number') {
            return group(value) + ' ms' + (value >= 1000 ? ' (' + PSL.dur(value) + ')' : '');
        }
        if (typeof value === 'boolean') return value ? 'yes' : 'no';
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
        if (value > 0 && pct < 0.1) return String(value);
        return (Math.round(pct * 10) / 10) + ' %';
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
            ruledOverline('// new analysis'),
            el('h1', {class: 'page-title', text: 'Run an analysis'})
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
            section.appendChild(el('div', {class: 'empty-state', text: 'This Hub has no configured source.'}));
            return section;
        }
        if (runSources().length === 0) {
            section.appendChild(el('div', {
                class: 'empty-state',
                text: 'No grant lets you start an analysis on this Hub. An administrator grants access in the '
                    + 'Hub\'s configuration.'
            }));
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
            head: '// prefer your terminal?',
            sub: 'The same run, spelled out.',
            help: 'The Hub runs this same binary. What is missing here is the JSON output and the '
                + 'second command that renders it, which exist so the Hub can build a dashboard. '
                + 'A terminal does not need either.',
            spell: function (shellId) {
                return PSL.analysisCommand(source, request, shellId);
            },
            copyLabel: 'Copy the analysis command',
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
                'This is the same request the button above sends, written as the engine\'s own '
                + 'arguments. It runs wherever perf-sentinel is installed and does not pass through '
                + 'this Hub: no worker slot, no queue, and no report kept here for '
                + state.status.limits.report_retention_hours + ' hours.',
                'It prints its findings to the terminal. There is no dashboard at the end of it and no '
                + 'link to share, which is the trade for not spending a worker.',
                trace
                    ? 'An ID resolves to exactly one trace, so the engine takes neither a window nor a '
                    + 'trace cap here, exactly as the form above stops offering them.'
                    : null,
                stepsBlock([
                    engineNeed(el('span', {})),
                    !trace && !state.form.service.trim()
                        ? 'Fill in the service name above. The command carries an empty one as it stands, '
                        + 'and the engine refuses it as it stands.'
                        : null,
                    source.auth_header_name ? tokenStep(source) : null,
                    changed > 0
                        ? 'Put the `perf-sentinel.toml` below next to where you run the command. '
                        + (changed === 1 ? 'The threshold you moved has' : 'The thresholds you moved have')
                        + ' no command-line flag, so the engine reads '
                        + (changed === 1 ? 'it' : 'them') + ' from that file.'
                        : null
                ])
                // The note that used to say which shell this was quoted for: the tabs
                // above the line say it, and they let the reader take the other one.
            ]
        })];

        if (changed > 0) {
            panels.push(terminalBlock({
                head: '// perf-sentinel.toml',
                sub: 'Only the thresholds you changed.',
                text: PSL.detectionToml(state.form.detection),
                copyLabel: 'Copy the perf-sentinel.toml fragment',
                download: 'perf-sentinel.toml',
                notes: [
                    'Every threshold this file leaves out keeps the engine\'s own default, and the Hub only '
                    + 'records a value that actually departs from one. A run launched from the button '
                    + 'above carries the same numbers, so the two are comparable with each other.',
                    'Put it in the directory you run the command from. The `-c` above makes it required, '
                    + 'so a run that cannot find it stops instead of quietly falling back to the '
                    + 'engine\'s own defaults, which are the numbers you just moved away from.'
                ]
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
            el('div', {
                text: 'The Hub is not answering. This is the Hub itself and not any one source, so nothing '
                    + 'can be launched from here until it is back. Reload once it responds again.'
            })
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
            ? PSL.dur(handoff.toMs - handoff.fromMs) + ' long.'
            : PSL.dur(handoff.atMs - handoff.fromMs) + ' before it to '
            + PSL.dur(handoff.toMs - handoff.atMs) + ' after.';
        // Named the way the incidents screen names it, then the reach of the run
        // said plainly: an analysis takes a service and no namespace, so one
        // taken from a namespaced incident is wider than the row it came from.
        const name = handoff.namespace ? handoff.namespace + '/' + handoff.service : handoff.service;
        // `other` is the daemon's catch-all, and "the other of cart-svc" is not
        // a sentence, so it reads as the incident a missing kind reads as.
        const kind = handoff.kind === 'other' ? null : PSL.INCIDENT_KIND_LABEL[handoff.kind];
        // An episode's window names no incident: it is said as the episode it came from.
        const origin = handoff.pairKey
            ? 'a correlation episode of ' + name + ' from the correlations screen, '
            : 'the ' + (kind || 'incident') + ' of ' + name + ' from the incidents screen, ';
        return el('div', {class: 'banner', 'data-tone': daemon ? 'warn' : 'info'}, [
            daemon ? warningGlyph(16) : infoGlyph(16),
            el('div', {
                text: 'Window of ' + origin + span
                    + (handoff.namespace
                        ? ' An analysis takes a service and no namespace, so this one covers '
                        + handoff.service + ' in every namespace.'
                        : '')
                    + (daemon ? ' A daemon takes no window. Pick a trace backend on the left to run it.' : '')
            })
        ]);
    }

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

    function sourcePanel() {
        return el('div', {class: 'card source-panel'}, [
            el('div', {class: 'panel-head'}, [
                el('h2', {class: 'overline', text: '// source'}),
                el('span', {
                    class: 'panel-head-source',
                    text: PSL.sourceCountLabel(runSources().length, (state.sources || []).length)
                })
            ]),
            radioKeys(el('div', {class: 'source-list', role: 'radiogroup', 'aria-label': 'Source'},
                sourceRows()), 'source'),
            el('p', {class: 'panel-note'}, [
                el('span', {class: 'panel-note-rule', 'aria-hidden': 'true'}),
                el('span', {
                    text: 'A dashed outline marks a value the source declares about itself. The Hub never '
                        + 'measures it. A misconfigured deployment can label production as staging.'
                })
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
        return [sourceGroupLabel('daemons')].concat(
            radios(kinds.daemons), [sourceGroupLabel('trace backends')], radios(kinds.backends));
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
                        ? 'reachable'
                        : 'unreachable ' + PSL.dur(now - source.unreachable_since_ms)
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
            'aria-label': source.name + ', ' + (source.reachable ? 'reachable' : 'unreachable'),
            'aria-describedby': metaId
        }, [el('span', {class: 'source-dot'}), el('span', {}, [line1, line2])]);
        button.addEventListener('click', function () {
            selectSource(source.id);
        });
        return button;
    }

    function producerLabel(source) {
        if (source.producer_version) return 'producer ' + source.producer_version;
        return source.kind === 'daemon' ? 'producer unknown' : 'no producer version';
    }

    function parametersPanel() {
        const source = selectedSource();
        if (!source) {
            return el('div', {class: 'card params-panel'}, [
                el('div', {class: 'empty-state', text: 'Pick a source to see what it takes.'})
            ]);
        }

        const head = el('div', {class: 'panel-head'}, [
            el('h2', {class: 'overline', text: source.kind === 'daemon' ? '// parameters' : '// query'}),
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
                el('p', {text: 'No parameters. A daemon snapshot is whatever it holds in memory right now.'}),
                el('p', {
                    class: 'notice-sub',
                    text: 'The window is its ring buffer, so there is nothing to widen. What it holds and how '
                        + 'it is configured are on its row in Sources.'
                }),
                selectedSource() && PSL.permits(selectedSource(), 'sources.read') ? sourcesRowLink() : null
            ])
        ]);
    }

    /** The label promises an open row, so the row arrives open: the sources
     screen reads the flag and runs the first read itself. */
    function sourcesRowLink() {
        const link = el('a', {class: 'pill-button pill-sm', href: '#/sources', text: 'Open its row on Sources'});
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
            nodes.push(field('Trace ID', traceInput()));
            nodes.push(el('p', {
                class: 'field-note',
                text: 'An ID resolves to exactly one trace, so neither the window nor the trace cap applies.'
            }));
            return nodes;
        }

        nodes.push(field('Service name', serviceInput()));
        nodes.push(field('Time range', rangeControl(source), state.form.rangeMode === 'absolute'
            ? 'absolute, fixed at submission'
            : 'relative to the moment the run starts'));
        nodes.push(maxTracesBlock());
        return nodes;
    }

    function modeSwitch() {
        const group = el('div', {class: 'segmented', role: 'radiogroup', 'aria-label': 'Selection mode'});
        [['service', 'Service'], ['trace', 'Trace ID']].forEach(function (entry) {
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
        return field('Select traces by', radioKeys(group, 'mode'), 'one or the other, never both');
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
            return PSL.dtHuman(state.form.fromMs) + ' → ' + PSL.dtHuman(state.form.toMs);
        }
        return 'Last ' + PSL.humanDur(state.form.lookback);
    }

    /** The span, and the argument the run will actually carry. */
    function rangeWire() {
        const span = PSL.dur(windowSpanMs());
        return state.form.rangeMode === 'absolute'
            ? span + ' · from_ms/to_ms'
            : span + ' · lookback = ' + state.form.lookback;
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
            'aria-label': 'Time range, ' + windowLabel(),
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
            notes.push(consequence('A wider window returns no more data. The run still stops at the trace '
                + 'cap, so the result is a sample spread over the period rather than the period itself.'));
        }
        if (spanMs > 7 * 86400000) {
            notes.push(consequence('The whole scan has to finish inside the '
                + (state.status.limits.analysis_timeout_seconds) + '-second ceiling, which is usually the '
                + 'limit met first. Expect a timeout rather than a result.'));
        }
        if (source.retention_hours != null && spanMs > source.retention_hours * 3600000) {
            notes.push(consequence('This source declares it keeps ' + PSL.dur(source.retention_hours * 3600000)
                + ' of traces. A window beyond that comes back short, or is refused as '
                + 'source_rejected_request.', 'warn'));
        } else if (source.retention_hours == null && spanMs > 86400000) {
            notes.push(consequence('Nobody declared how far back this source keeps traces, so the Hub '
                + 'cannot tell whether it can answer this window at all.'));
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
        const apply = el('button', {type: 'button', class: 'picker-apply', text: 'Apply range'});

        function readAbsolute() {
            const start = Date.parse(from.value);
            const end = Date.parse(to.value);
            const ordered = Number.isFinite(start) && Number.isFinite(end) && start < end;
            const past = Number.isFinite(end) && end <= Date.now();
            const valid = ordered && past;
            note.textContent = !ordered
                ? 'The start must come before the end.'
                : !past
                    ? 'The end cannot be in the future.'
                    : PSL.dur(end - start) + ' selected';
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
            dateField('From', from),
            dateField('To', to),
            el('div', {class: 'picker-apply-row'}, [apply, note]),
            el('div', {class: 'picker-rule'}),
            el('p', {class: 'overline', text: 'Custom relative'}),
            customRelativeRow()
        ]);

        const right = el('div', {class: 'picker-pane picker-right'}, [
            el('p', {class: 'overline picker-quick-head', text: 'Quick ranges'}),
            el('div', {class: 'picker-quick'}, QUICK_RANGES.map(function (value) {
                const active = state.form.rangeMode === 'relative' && state.form.lookback === value;
                const button = el('button', {
                    type: 'button',
                    class: 'picker-quick-item',
                    'aria-current': active ? 'true' : null,
                    text: 'Last ' + PSL.humanDur(value)
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
        const units = el('div', {class: 'segmented segmented-sm', role: 'radiogroup', 'aria-label': 'Unit'});
        ['m', 'h', 'd'].forEach(function (unit) {
            const button = el('button', {
                type: 'button',
                role: 'radio',
                'aria-checked': state.form.customUnit === unit ? 'true' : 'false',
                text: unit
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

        const apply = el('button', {type: 'button', class: 'pill-button pill-sm', text: 'Apply'});
        apply.addEventListener('click', function () {
            const quantity = Math.max(1, Number(qty.value) || 1);
            state.form.customQty = quantity;
            applyRange('relative', {lookback: quantity + state.form.customUnit});
        });

        return el('div', {class: 'picker-custom'}, [
            el('span', {class: 'picker-custom-lead', text: 'Last'}),
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
            'aria-label': 'Max traces'
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
                el('label', {for: 'traces-number', text: 'Max traces'}),
                helpDot('This and the window travel to the backend in one search, the window as its '
                    + 'time bounds, this as its limit. It is a ceiling and not a target, so a window '
                    + 'holding fewer traces returns fewer. When it holds more, the backend picks which '
                    + 'ones and they are not an even spread, so narrow the window when you need a given '
                    + 'stretch covered. Traces come back whole rather than sampled, so this number bounds '
                    + 'what the run costs.', 'max traces'),
                el('span', {class: 'field-gloss', text: 'how much comes back, not how far back'})
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

        return sinkBlock('// what your own runs weighed', 'Measured on this source, not predicted.',
            past.map(function (run) {
                const traces = typeof run.request.max_traces === 'number' ? run.request.max_traces : 100;
                return [PSL.bytes(run.result.report_bytes),
                    traces + ' traces, ' + run.result.findings + ' findings, '
                    + PSL.dur(Date.now() - (run.finished_at_ms || run.created_at_ms)) + ' ago'];
            }));
    }

    function capNote(band, cap) {
        if (band.key === 'invalid') return 'at least 1';
        if (band.key === 'over') return 'above the hard cap of ' + cap + ', the service will reject this';
        return 'hard cap ' + cap;
    }

    /**
     * The bands that exist at this cap, each with the boundary it ends at and
     * its share of the rule. A cap at or below a boundary means that band never
     * occurs: it is dropped rather than drawn at zero width under a label
     * repeating the cap.
     */
    function bands(cap) {
        const inner = [['ok', 500, 'safe'], ['warn', 1200, 'heavy']]
            .filter(function (band) {
                return band[1] < cap;
            });
        // The stripe that ends at the cap takes the tone of whichever band the cap
        // itself falls in, which is the one after the last inner boundary kept.
        // Always painting it crit would turn a Hub capped at 500 into an entirely
        // red rule over a range that is all comfortable.
        const TONES = ['ok', 'warn', 'crit'];
        const kept = inner.concat([[TONES[inner.length], cap, 'cap']]);

        let previous = 0;
        return kept.map(function (band) {
            const share = (band[1] - previous) / cap;
            previous = band[1];
            return {
                tone: band[0],
                label: group(band[1]) + ' ' + band[2],
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
        return whole.replace(/\B(?=(\d{3})+(?!\d))/g, '\u202f') + (point < 0 ? '' : text.slice(point));
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
            ['550 KB', 'The floor. Fonts, styles and the dashboard itself, present in every report '
            + 'whether it found one problem or none.'],
            ['every finding', 'Every run this Hub executes renders every finding it found, at any '
            + 'size. The count on the dashboard is the count that was found.'],
            [String(embeddedCap()) + ' trees', 'Span trees embedded, for the findings with the highest '
            + 'aggregate impact. The rest open without a tree and say so, with the trace id to read '
            + 'that one on its own, which the Trace ID mode above runs directly. Set by whoever '
            + 'operates this Hub.'],
            ['25', 'Hard cap on the top offenders embedded for the Carbon tab, whatever the run size. '
            + 'The full ranking is still computed, only the embed is capped.'],
            ['no ceiling', 'The file has no size target of its own once every finding is kept. A run '
            + 'that finds a great deal produces a report that takes a moment to open.']
        ];
        // Folded until the reader opens it, like every other fold in the product,
        // and remembered from then on.
        return sinkBlock('// what comes back, and what it caps',
            'From the sink and this Hub\'s settings, not predictions.', rows, {
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
            'I accept a long run and a heavy report.',
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
                    text: source.name + ' runs ' + source.producer_version + ', ' + skew.label + ' the '
                        + engine + ' binary embedded in the Hub.'
                }),
                el('p', {
                    class: 'notice-block-body',
                    text: behind
                        ? 'perf-sentinel is pre-1.0, so detectors change between minors. A detector added in '
                        + engine + ' does not run on this producer at all, and its absence looks exactly like '
                        + 'a clean service. Read a low finding count from this source as unmeasured, not as healthy.'
                        : 'Envelopes are additive, so nothing breaks. Findings from a detector this Hub does not '
                        + 'know about arrive unnamed. The Hub compares two version strings and cannot know '
                        + 'whether this minor changed detection at all.'
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
            el('span', {text: 'Get ' + engine + ': '}),
            el('a', {
                class: 'notice-block-link',
                href: PSL.releaseUrl(engine),
                target: '_blank',
                rel: 'noopener noreferrer',
                text: 'release notes and binaries'
            })
        ]);
        if (source.kind !== 'daemon') return line;
        line.appendChild(el('span', {text: ', or the chart this daemon is deployed from, '}));
        line.appendChild(el('a', {
            class: 'notice-block-link',
            href: PSL.CHART_PAGE,
            target: '_blank',
            rel: 'noopener noreferrer',
            text: 'every chart version and the engine it ships'
        }));
        line.appendChild(el('span', {text: '. The chart itself is '}));
        // A coordinate for helm, not a link: the scheme is not one a browser opens.
        line.appendChild(el('code', {class: 'code-inline', text: PSL.CHART_COORDINATE}));
        line.appendChild(el('span', {text: '.'}));
        return line;
    }

    function unreachableNotice(source) {
        const text = el('div', {class: 'notice-block-text'}, [
            el('p', {
                class: 'notice-block-title',
                text: source.name + ' has been unreachable for ' + PSL.dur(Date.now() - source.unreachable_since_ms) + '.'
            })
        ]);
        if (source.last_success_ms) {
            text.appendChild(el('p', {
                class: 'notice-block-body',
                text: 'Last successful contact ' + PSL.dur(Date.now() - source.last_success_ms) + ' ago.'
            }));
        }
        if (source.last_error_code) {
            text.appendChild(el('p', {class: 'notice-block-body'}, [
                el('span', {text: 'The last attempt returned '}),
                el('span', {class: 'code-inline', text: source.last_error_code}),
                proseInto(el('span'), ': ' + (PSL.READ_ERRORS[source.last_error_code]
                    || PSL.ERRORS[source.last_error_code] || 'the Hub could not reach it.'))
            ]));
        }
        text.appendChild(el('p', {
            class: 'notice-block-body',
            text: 'Running now will consume a worker slot and will almost certainly end with the same code.'
        }));
        text.appendChild(checkbox(
            state.form.ackUnreachable,
            'Run it anyway',
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
            [String(limits.max_traces_cap), 'traces', 'Hard cap per run', 'The service rejects anything above it.'],
            [String(limits.analysis_timeout_seconds), 's', 'Timeout', 'Then the run is killed, marked timeout.'],
            [String(state.status.workers), 'workers',
                queue === 1 ? '1 job queued now' : queue + ' jobs queued now',
                'That many runs at a time across the whole Hub.'],
            [String(limits.report_retention_hours), 'h', 'Report retention', 'Then the file is deleted. Links die.']
        ];
        return el('section', {class: 'card cost'}, [
            el('div', {class: 'cost-head'}, [
                el('h2', {class: 'overline', text: '// what this run costs'}),
                el('span', {class: 'cost-sub', text: 'Reported by the service, not assumed.'})
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
        if (!source) return 'Pick a source.';
        if (!state.status.engine_version) return 'This Hub has no analysis engine configured.';
        if (!source.reachable && !state.form.ackUnreachable) return 'Confirm you want to run against an unreachable source.';
        if (source.kind === 'daemon') return null;
        if (state.form.mode === 'trace') {
            return state.form.traceId.trim() ? null : 'Enter a trace ID.';
        }
        if (!state.form.service.trim()) return 'Enter a service name.';
        const band = PSL.weightBand(state.form.maxTraces, tracesCap());
        if (band.key === 'over') return 'The trace cap is above what the service accepts.';
        if (band.key === 'invalid') return 'A run needs at least one trace.';
        return band.needsAck && !state.form.ackHeavy ? 'Confirm the long run and heavy report.' : null;
    }

    /** Restates the request in a sentence, so the button is not a leap of faith. */
    function submitSentence() {
        const source = selectedSource();
        if (!source) return '';
        if (source.kind === 'daemon') {
            return 'Takes a snapshot of what ' + source.name + ' holds in memory. No query is sent to a '
                + 'trace backend. ' + queuePhrase();
        }
        if (state.form.mode === 'trace') {
            return 'Fetches one trace by ID from ' + source.name + '. ' + queuePhrase();
        }
        return 'Reads up to ' + state.form.maxTraces + ' traces for '
            + (state.form.service.trim() || 'a service') + ' across '
            + (state.form.rangeMode === 'absolute' ? 'the selected window' : 'the last ' + PSL.humanDur(state.form.lookback))
            + ' of ' + source.name + '. ' + queuePhrase();
    }

    function queuePhrase() {
        const queue = state.status.queue_depth;
        if (queue === 0) return 'Nothing is queued ahead of it.';
        return queue === 1 ? 'Queued behind 1 job.' : 'Queued behind ' + queue + ' jobs.';
    }

    function submitRow() {
        const button = el('button', {type: 'button', class: 'submit', id: 'submit'}, [
            playGlyph(),
            el('span', {text: 'Run analysis'})
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
            if (!result.ok) throw new Error(result.payload.detail || 'The Hub refused the request.');
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
            el('span', {class: 'status-pill', 'data-status': key, text: key}),
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
            el('h1', {class: 'page-title', text: 'No analysis with that ID.'}),
            el('p', {
                class: 'page-sub',
                text: 'This Hub holds no analysis under it. The link may be mistyped, or come from another Hub.'
            }),
            el('div', {class: 'outcome-actions'}, [
                el('a', {class: 'action action-filled', href: '#/new', text: 'Run an analysis'}),
                el('a', {class: 'action action-secondary', href: '#/recent', text: 'All analyses'})
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
            el('span', {text: 'All analyses'})
        ]);
    }

    /** Headline and sub-line per state, in the source's own terms. */
    function runView(run, key) {
        if (key === 'queued') {
            return {
                headline: 'Waiting for a worker.',
                sub: 'Every worker is busy. Nothing has been read from ' + run.source_name
                    + ' yet, so nothing has been spent.'
            };
        }
        if (key === 'running') {
            return {
                headline: 'Reading ' + run.source_name + '.',
                sub: 'A worker holds this job. The next thing that happens is a result or a failure, with '
                    + 'nothing in between.'
            };
        }
        if (key === 'empty') {
            return {
                headline: 'It succeeded, and there is nothing in it.',
                sub: 'This is not a failure and not an error. The source answered correctly, and the answer '
                    + 'was zero traces.'
            };
        }
        if (key === 'succeeded') {
            const result = run.result || {};
            const caveats = (result.warnings || []).length;
            return {
                headline: caveats > 0
                    ? result.findings + ' findings, and ' + (caveats === 1 ? 'a caveat' : caveats + ' caveats')
                    + ' you should read first.'
                    : result.findings + ' findings.',
                sub: function () {
                    return 'The report is ready and will be deleted in '
                        + PSL.durPrecise(run.expires_at_ms - Date.now()) + '.';
                }
            };
        }
        if (key === 'interrupted') {
            return {
                headline: 'The service restarted while this was running.',
                sub: 'It stopped after ' + PSL.dur((run.finished_at_ms || 0) - (run.started_at_ms || run.created_at_ms))
                    + ' of work. This is a resumption, not an error to investigate.'
            };
        }
        if (key === 'expired') {
            return {
                headline: 'This report was deleted.',
                sub: function () {
                    return 'Reports live ' + state.status.limits.report_retention_hours + ' hours. This one expired '
                        + PSL.durPrecise(Date.now() - run.expires_at_ms) + ' ago and the file is gone.';
                }
            };
        }
        return {
            headline: 'Failed: ' + String(run.error_code || 'internal').replace(/_/g, ' ') + '.',
            sub: 'The Hub does not expose the process\'s error output by design. It gives one code out of '
                + 'eight, and this is what that code means.'
        };
    }

    /**
     * A receipt, not a feed. Every line is a timestamp the Hub wrote: the design
     * calls for a `dequeued` line too, but this service records one instant for
     * dequeue and start, and inventing a second would be interpolation.
     */
    function eventLog(run, key) {
        const rows = [logRow(run.created_at_ms, 'accepted', 'the request was validated and queued', 'muted')];
        if (run.started_at_ms) {
            rows.push(logRow(run.started_at_ms, 'started',
                run.kind === 'daemon' ? 'reading the daemon\'s in-memory store' : 'reading ' + PSL.KIND_LABEL[run.kind],
                'brand'));
        }
        if (key === 'running') rows.push(logRow(null, 'running', 'no further event until the engine returns', 'brand'));
        if (key === 'queued') rows.push(logRow(null, 'waiting', 'every worker is busy, nothing has been read yet', 'muted'));
        if (key === 'succeeded' || key === 'empty') {
            rows.push(logRow(run.finished_at_ms, 'succeeded',
                'report written, retained ' + state.status.limits.report_retention_hours + ' h', 'ok'));
        }
        if (key === 'failed') rows.push(logRow(run.finished_at_ms, 'failed', run.error_code, 'crit'));
        if (key === 'interrupted') {
            rows.push(logRow(run.finished_at_ms, 'interrupted',
                'the Hub restarted, the run was abandoned and not replayed', 'info'));
        }
        if (key === 'expired') {
            rows.push(logRow(run.finished_at_ms, 'succeeded', 'report written', 'muted'));
            rows.push(logRow(run.expires_at_ms, 'deleted',
                'retention reached, the report no longer exists', 'muted'));
        }

        return el('section', {class: 'card log-card', 'aria-label': 'Service events'}, [
            el('div', {class: 'log-head'}, [
                el('h2', {class: 'overline', text: '// service events'}),
                el('span', {class: 'log-head-note', text: 'Only what the Hub actually recorded.'})
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
        if (key === 'running' || key === 'queued') {
            return 'The engine reports nothing between start and finish. There is no percentage to show '
                + 'and no arrival time to predict, so this screen shows neither. Only the events above, the '
                + 'time spent, and the ceiling at which the service gives up. Expect one more line, not a stream.';
        }
        const instant = run.finished_at_ms && (run.finished_at_ms - run.created_at_ms) < 10000;
        return instant
            ? 'This run was read and finished in one step, so every line above was written at once. It is '
            + 'a receipt of what happened, not a feed.'
            : 'Every line above is a timestamp the Hub wrote. Nothing here is interpolated.';
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
            el('h2', {class: 'overline overline-p', text: '// elapsed'}),
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
        if (key === 'queued') {
            return 'The ' + seconds + '-second ceiling starts when a worker picks the job up, not now.';
        }
        if (key !== 'running') return 'Total time the run occupied a worker.';
        const left = seconds * 1000 - elapsedMs;
        return left <= 0
            ? 'Past the ' + seconds + ' s ceiling. The run should already have been killed and marked timeout.'
            : 'Hard stop at ' + seconds + ' s, then the run is killed and marked timeout. '
            + PSL.dur(left) + ' of ceiling left.';
    }

    function requestCard(run) {
        const request = run.request || {};
        const facts = [['source', run.source_name, 'ui'], ['type', PSL.KIND_LABEL[run.kind] || run.kind, 'mono']];
        ['service', 'trace_id', 'lookback', 'max_traces'].forEach(function (name) {
            if (request[name] != null) facts.push([name, String(request[name]), 'mono']);
        });
        if (request.from_ms) facts.push(['window', PSL.dtHuman(request.from_ms) + ' → ' + PSL.dtHuman(request.to_ms), 'mono']);
        Object.keys(request.detection || {}).forEach(function (name) {
            facts.push([name, String(request.detection[name]), 'warn']);
        });
        if (!request.service && !request.trace_id) facts.push(['parameters', 'none', 'muted']);
        facts.push(['requested by', run.requested_by, 'mono']);
        facts.push(['detected by', run.producer_version
            ? PSL.detector(run.kind) + ' ' + run.producer_version
            : 'not yet known', PSL.skew(run.producer_version) ? 'warn' : 'mono']);
        facts.push(['expires', expiryText(run),
            run.expires_at_ms && run.expires_at_ms < Date.now() ? 'crit' : 'mono',
            function () {
                return expiryText(run);
            }]);

        return el('section', {class: 'request'}, [
            el('h2', {class: 'overline overline-p', text: '// request'}),
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
        if (!run.expires_at_ms) return 'not until it succeeds';
        const delta = run.expires_at_ms - Date.now();
        return delta > 0 ? 'in ' + PSL.durPrecise(delta) : PSL.durPrecise(-delta) + ' ago';
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
            panel.appendChild(el('div', {class: 'outcome-warning'}, [
                el('span', {class: 'outcome-warning-kind', text: warning.kind}),
                el('span', {class: 'outcome-warning-message', text: warning.message})
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
                tone: result.quality_gate_passed ? 'ok' : 'crit', title: 'result',
                body: result.quality_gate_passed
                    ? 'The quality gate passed. The dashboard holds the full detail.'
                    : 'The quality gate did not pass. The dashboard holds the full detail.',
                counts: [
                    [result.quality_gate_passed ? 'PASS' : 'FAIL', 'quality gate',
                        result.quality_gate_passed ? 'ok' : 'crit'],
                    [String(result.findings), result.kept_findings == null ? 'findings' : 'found',
                        result.critical > 0 ? 'crit' : result.warning > 0 ? 'warn'
                            : result.info > 0 ? 'info' : 'ok'],
                    [String(result.critical), 'critical', 'crit'],
                    [String(result.warning), 'warning', 'warn'],
                    [String(result.info), 'info', 'info'],
                    [String(result.traces_analyzed), 'traces read', 'text']
                ],
                warnings: result.warnings,
                primary: {label: 'Open the dashboard', href: '#/report/' + run.id, filled: true},
                note: function () {
                    return 'Opens on this origin. The link dies in '
                        + PSL.durPrecise(run.expires_at_ms - Date.now()) + '.';
                }
            };
        }
        if (key === 'empty') {
            return {
                tone: 'warn', title: 'empty result',
                body: run.source_name + ' had nothing for the engine to analyse. The report exists, and it is '
                    + 'blank. Opening it will show an empty dashboard. That is the expected outcome, not a '
                    + 'rendering fault.',
                counts: [
                    [result.quality_gate_passed ? 'PASS' : 'FAIL', 'quality gate', 'muted'],
                    [String(result.findings), 'findings', 'warn'],
                    [String(result.traces_analyzed), 'traces read', 'warn']
                ],
                warnings: result.warnings,
                primary: rerunAction('Wait and run it again', false),
                secondary: {label: 'Open the blank dashboard anyway', href: '#/report/' + run.id},
                note: 'A quality gate that passes on zero traces has not measured anything.'
            };
        }
        if (key === 'failed') {
            return {
                tone: 'crit', title: run.error_code || 'internal',
                body: run.source_name + ': ' + (PSL.ERRORS[run.error_code] || 'it failed for an unnamed reason.'),
                primary: rerunAction('Run it again', false),
                secondary: {label: 'Check the source', href: '#/sources'},
                note: 'Nothing was stored, so nothing expires.'
            };
        }
        if (key === 'interrupted') {
            return {
                tone: 'info', title: 'resume',
                body: 'The Hub never replays an interrupted run on its own. A silent retry could fire a second '
                    + 'heavy query at ' + run.source_name + ' without anyone asking for it, so the decision stays '
                    + 'yours. The parameters are unchanged and ready to send again.',
                primary: {
                    label: 'Resume with the same parameters',
                    action: function (button) {
                        resubmit(run, button);
                    },
                    filled: true
                },
                note: PSL.argsLine(run)
            };
        }
        return {
            tone: 'muted', title: 'expired',
            body: 'Retention is not configurable from here. Running the same analysis again produces a new '
                + 'report with a new clock. It will not reproduce the old one, because the source has moved '
                + 'on since then.',
            primary: rerunAction('Run it again', false),
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
            el('span', {class: 'outcome-warning-kind', text: 'trimmed'}),
            el('span', {
                class: 'outcome-warning-message',
                text: result.findings + ' findings were found and ' + result.kept_findings
                    + ' are in the report. The sink dropped the rest to fit, critical last, so what '
                    + 'survived is what mattered most.'
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
                throw new Error(result.payload.detail || 'The Hub refused the resubmission.');
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
            el('p', {text: 'The Hub is not answering. This page keeps what it last read and asks again.'})
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
        n_plus_one_min_occurrences: 'How many near-identical queries in one trace count as an N+1. '
            + 'Raise it and smaller loops stop being reported at all.',
        window_duration_ms: 'How close together those queries have to be. A shorter window splits one '
            + 'slow loop into several groups that each fall under the count.',
        slow_query_threshold_ms: 'Above this, one operation is called slow.',
        slow_query_min_occurrences: 'How many times a slow template has to appear before it is worth '
            + 'reporting. One slow query stays invisible below this.',
        max_fanout: 'Child spans under one parent before it counts as excessive fanout. The engine '
            + 'warns outside 5 to 1 000: too low floods the list, too high hides real fan-outs.',
        chatty_service_min_calls: 'Outbound HTTP calls in one trace before a service is called chatty. '
            + 'Critical fires at three times this.',
        pool_saturation_concurrent_threshold: 'Peak concurrent SQL spans on one service before the '
            + 'connection pool is called at risk. Set it to the pool size you actually run.',
        serialized_min_sequential: 'Sequential independent calls under one parent before they are '
            + 'worth parallelising.',
        sanitizer_aware_classification: 'How a run of identical parameterised queries is read once the '
            + 'agent has hidden their literals. `auto` calls it an N+1 on the ORM scope alone, `strict` '
            + 'also wants the timings to spread, `never` leaves it a redundant query, `always` reports '
            + 'an N+1.',
        sanitizer_aware_min_cv: 'How much those timings have to spread (standard deviation over '
            + 'mean) before `strict` or `auto` call the run an N+1 rather than a cached repeat. The same '
            + 'bar reads a repeated HTTP call. Raise it on a jittery runtime such as PHP-FPM, where '
            + 'repeats of one cached query spread past 0.5.'
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
        badge.textContent = count === 1 ? '1 changed' : count + ' changed';
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
            el('span', {class: 'visually-hidden', text: 'Warning, expert settings.'}),
            el('span', {class: 'overline'}, [
                el('span', {class: 'over-warn', text: '// advanced users only'}),
                document.createTextNode(' \u00b7 what the analysis looks for')
            ]),
            el('span', {id: 'advanced-count', class: 'advanced-count', hidden: 'hidden'})
        ]);

        const body = el('div', {class: 'advanced-body'}, [
            el('section', {class: 'notice-block', 'data-tone': 'warn'}, [
                warningGlyph(17),
                el('div', {class: 'notice-block-text'}, [
                    el('p', {
                        class: 'notice-block-title',
                        text: 'For operators who know what these thresholds do.'
                    }),
                    el('p', {
                        class: 'notice-block-body',
                        text: 'Moved carelessly, they hide real problems or flood the report with noise, and '
                            + 'which direction does which differs per threshold. If you are not sure, leave them.'
                    })
                ])
            ]),
            el('p', {
                class: 'advanced-lead',
                text: 'These are the engine\'s detection thresholds. They decide what counts as a problem, '
                    + 'not how the report is written: raising one does not make the run lighter, it makes the '
                    + 'engine stop reporting the smaller cases. A run records the ones you changed, and the '
                    + 'recent list flags counts that came from different thresholds, because they are not '
                    + 'comparable.'
            })
        ]);

        // One button for the lot, beside the count so the two agree at a glance.
        const resetAll = el('button', {
            type: 'button',
            id: 'advanced-reset',
            class: 'pill-button pill-sm advanced-reset',
            text: 'Reset every threshold'
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
        const label = 'Put ' + knob.name + ' back to ' + knob.default;
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
                el('span', {class: 'knob-default', text: 'default ' + knob.default})
            ]),
            proseInto(el('span', {class: 'knob-body'}), DETECTION_COPY[knob.name] || ''),
            el('div', {class: 'knob-controls'}, [input, reset])
        ]);
    }

    // ------------------------------------------------------ screen: incidents

    const INCIDENT_PAGE = 100;
    // In the order the daemon's own monitor tab prints them, so the two
    // surfaces never disagree about what comes first. The TUI prints
    // ns/service in one cell, this table gives the namespace its own.
    const INCIDENT_COLUMNS = ['Started', 'Namespace', 'Service', 'Kind', 'Ended', 'Findings', 'Capture', 'Source'];
    const INCIDENT_COLUMNS_RIGHT = ['Started', 'Ended', 'Findings'];
    const INCIDENT_CAPTURE = {
        complete: 'The ring still reached the whole window when the incident was frozen: what is here is what fired.',
        partial: 'The ring had already evicted part of the window when the incident was frozen. Findings that fired '
            + 'earlier in the window are missing here, and the daemon\'s NDJSON archive may still hold them.',
        empty: 'The ring held nothing when the incident was frozen.'
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
            ruledOverline('// incidents'),
            el('h1', {class: 'page-title', text: 'Incidents the daemons recorded'}),
            el('p', {class: 'page-sub'}, [
                document.createTextNode('What was already burning when your alerting posted a restart, an OOM kill '
                    + 'or a deploy: each row carries the findings its daemon froze from the minutes before. '),
                helpDot('The daemon is the author: the Hub copies its record, re-derives nothing, and keeps the '
                    + 'copy after the daemon\'s own ring has let it go. Opening this screen reads every daemon, so '
                    + 'the rows are what the fleet holds now, and under the table is when each one was last read. '
                    + 'A deploy is posted to freeze what was already firing before the rollout, so a restart it '
                    + 'causes is not read as a crash.', 'incidents')
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
                el('div', {text: INCIDENT_ERROR_TEXT[state.incidentsError]})
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
                el('p', {class: 'empty-title', text: 'No incident recorded.'}),
                proseInto(el('p'), 'An incident exists only when a daemon with `[daemon.incidents]` enabled '
                    + 'receives one from the operator\'s alerting. Nothing here means none was posted'
                    + (Object.values(state.incidentFilter).some(Boolean) ? ' for this filter' : '')
                    + ', or that no daemon read just now publishes them yet.')
            ]));
            appendCopyNote(section);
            return section;
        }
        section.appendChild(el('div', {class: 'sources-wrap'}, [incidentsTable(state.incidents)]));
        if (!state.incidentsDone) {
            const older = el('button', {type: 'button', class: 'pill-button'}, [
                svg([['path', {d: 'M12 5v14M5 12l7 7 7-7'}]], 14),
                el('span', {text: 'Load older incidents'})
            ]);
            older.addEventListener('click', function () {
                loadOlderIncidents(older);
            });
            section.appendChild(el('p', {class: 'sources-note'}, [older]));
        }
        appendCopyNote(section);
        section.appendChild(el('p', {
            class: 'sources-note',
            text: 'Started and Ended are the alerting\'s own stamps, relayed by the daemon. The Hub keeps an incident '
                + 'for its findings retention, on its own clock, and a daemon that answers 404 on this route runs a '
                + 'release before 0.20.0.'
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
                        el('span', {
                            text: source.name + ' refused the Hub\'s key on its incidents route, so its '
                                + 'incidents are not here. Its findings are still collected. The daemon\'s '
                        }),
                        el('span', {class: 'code-inline', text: '[daemon] read_api_key'}),
                        el('span', {text: ' goes in this source\'s '}),
                        el('span', {class: 'code-inline', text: 'AuthHeaderValue'}),
                        el('span', {text: ', sent as '}),
                        el('span', {class: 'code-inline', text: 'X-API-Key'}),
                        el('span', {text: '.'})
                    ])
                ])
            ]);
        });
    }

    /**
     * One labelled select for the filter line. The first option is the whole
     * set, worded from the label, and `options` are [value, text] pairs.
     */
    function filterSelect(label, value, options, onChange, disabled) {
        const select = el('select', {
            class: 'refresh-select', 'aria-label': label, disabled: disabled ? 'disabled' : null
        }, [
            el('option', {value: '', text: 'every ' + label})
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
            el('span', {text: state.incidentsReading ? 'Reading the daemons' : 'Read the daemons now'})
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
        stale: 'The daemons were not read just now, so these rows are the copy the last read left. '
            + 'The line under the table says how old each one is. Read again in a moment.',
        unread: 'The Hub did not return the incidents, so nothing here is a reading of the fleet. '
            + 'This is the Hub itself, not any daemon.',
        page: 'The Hub is not answering, so the older rows are unknown. This is the Hub itself, '
            + 'not any daemon. Rows already on screen are the last page it did answer.'
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
            el('span', {text: 'Every row comes from a daemon, copied here when its ring was read. '})
        ].concat(daemons.map(function (source) {
            const text = function () {
                return PSL.incidentsCopy(source, Date.now());
            };
            // The title on the wrapper, not on the live span: the ticker copies
            // its own text into any title it finds on the node it rewrites.
            return el('span', {
                title: source.incidents_read_ms == null ? null : PSL.dtHuman(source.incidents_read_ms)
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
                text: name,
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
        row.appendChild(el('td', {'data-align': 'right', title: PSL.dtHuman(incident.at_ms)}, [
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
                text: 'after ' + PSL.dur(incident.ended_at_ms - incident.at_ms),
                title: PSL.dtHuman(incident.ended_at_ms)
            })
            : el('td', {'data-align': 'right'}, [el('span', {class: 'table-muted', text: 'still open'})]));
        row.appendChild(el('td', {'data-align': 'right', text: String(incident.finding_count)}));
        row.appendChild(el('td', {title: INCIDENT_CAPTURE[capture]}, [
            el('span', {class: capture === 'empty' ? 'table-muted' : null, text: capture})
        ]));
        row.appendChild(el('td', {}, [
            el('span', {class: 'table-strong', text: incident.source_name || incident.source_id}),
            el('span', {text: ' '}),
            el('span', {class: 'chip chip-declared', text: incident.environment || 'unknown'})
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
        return PSL.dur(Date.now() - incident.at_ms) + ' ago';
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
                + ', ' + PSL.dur(Date.now() - incident.at_ms) + ' ago'
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
                el('p', {class: 'daemon-loading', role: 'status', text: 'Reading the incident.'}),
                el('div', {class: 'skeleton', style: 'height:90px'})
            ]);
        }
        if (detail.error_code) {
            return el('div', {class: 'daemon-panel'}, [
                el('div', {class: 'banner', 'data-tone': 'crit'}, [
                    critGlyph(16),
                    el('p', {
                        text: detail.error_code === 'gone'
                            ? 'The Hub no longer holds this incident. Retention removed it between the listing and this read.'
                            : 'The Hub could not read this incident. Fold the row and open it again.'
                    })
                ])
            ]);
        }
        const capture = PSL.incidentCapture(detail);
        const findings = PSL.sortFindings(detail.findings || [], state.findingSort.key, state.findingSort.dir);
        // A link, so it opens in a new tab too. Its end is clamped to the moment it is followed.
        const analyse = el('a', {class: 'pill-button', href: PSL.incidentHandoffHash(detail, Date.now())}, [
            svg([['path', {d: 'M5 12h14M13 6l6 6-6 6'}]], 14),
            el('span', {text: 'Analyse this window'})
        ]);
        // The button comes after the note that describes the window, since it
        // is the action the note argues for. A bare row, so the panel's own
        // gap sets the distance and no margin adds a second one.
        return el('div', {class: 'daemon-panel'}, [
            el('p', {class: 'overline daemon-audience', text: '// frozen by the daemon'}),
            el('p', {
                class: 'daemon-source-note',
                text: INCIDENT_CAPTURE[capture] + ' The window ran from '
                    + PSL.dur(detail.at_ms - detail.window_from_ms) + ' before the incident to '
                    + PSL.dur(detail.window_to_ms - detail.at_ms) + ' after it, and a finding stamped after the '
                    + 'incident belongs to the replacement, not to what died.'
            }),
            el('div', {}, [analyse]),
            findings.length === 0
                ? el('p', {class: 'daemon-lead', text: 'The daemon froze no finding for this incident.'})
                : findingsFilter(detail),
            findings.length === 0 ? null : el('div', {class: 'sources-wrap'}, [incidentFindingsTable(findings, detail)]),
            incidentCorrelationsBlock(incident)
        ]);
    }

    /** Narrows the frozen findings in place, on their endpoint or template, without drawing the screen again. */
    function findingsFilter(incident) {
        const key = incidentKey(incident);
        const input = el('input', {
            type: 'search', class: 'input findings-filter', placeholder: 'endpoint or template',
            value: state.findingQuery[key] || '', autocomplete: 'off', spellcheck: 'false'
        });
        const count = el('span', {class: 'field-gloss', 'aria-live': 'polite'});
        const apply = function () {
            state.findingQuery[key] = input.value;
            const rows = document.querySelectorAll('[data-findings="' + CSS.escape(key) + '"] tbody tr');
            let shown = 0;
            Array.prototype.forEach.call(rows, function (row) {
                row.hidden = !PSL.findingMatches(FINDING_ROW.get(row), input.value);
                if (!row.hidden) shown += 1;
            });
            count.textContent = input.value.trim() === '' ? '' : shown + ' of ' + rows.length + ' shown';
        };
        input.addEventListener('input', apply);
        // Once the table beside it is in the page, so a query kept across a sort applies again.
        queueMicrotask(apply);
        const row = field('Filter the findings', input, null);
        row.appendChild(count);
        return row;
    }

    // Each table row's frozen finding, for the filter to test without reading it back from the cells.
    const FINDING_ROW = new WeakMap();

    function incidentFindingsTable(findings, incident) {
        // Without a single ack credential in the Hub, every link would open a page that can do nothing.
        const canAck = (state.sources || []).some(function (source) {
            return source.ack_relay;
        });
        const columns = ['Type', 'Severity', IMPACT_HEAD, 'Endpoint', 'Template', 'Seen', 'First seen'].concat(canAck ? ['Ack'] : []);
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
                    text: finding.severity || '?'
                })]),
                el('td', {'data-align': 'right', text: impact == null ? '—' : impact.toLocaleString('en')}),
                el('td', {class: 'table-mono', text: finding.source_endpoint || '?'}),
                // Whole, wrapped: the template is what tells two rows apart.
                el('td', {class: 'table-mono'}, [el('span', {class: 'findings-template', text: template || '—'})]),
                el('td', {'data-align': 'right', text: String(row.seen_count == null ? '?' : row.seen_count)}),
                el('td', {
                    title: PSL.dtHuman(row.first_seen_ms),
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
            text: 'No source in this Hub has an ack credential, so these findings are acknowledged in each daemon, not from here.'
        })]);
    }

    // The findings dashboard's own name for it, so the two read the same number the same way.
    const IMPACT_HEAD = 'Est. impact (ops)';

    const FINDING_SORT_KEY = {
        Type: 'type',
        Severity: 'severity',
        [IMPACT_HEAD]: 'impact',
        Endpoint: 'endpoint',
        Template: 'template',
        Seen: 'seen',
        'First seen': 'first'
    };

    /** A column head that sorts on click. Counts and severity read worst first, the rest A to Z. */
    function findingHead(name, incident) {
        const key = FINDING_SORT_KEY[name];
        const th = el('th', {scope: 'col', 'data-align': name === 'Seen' || name === IMPACT_HEAD ? 'right' : null});
        if (!key) {
            th.textContent = name;
            return th;
        }
        const sort = state.findingSort;
        const active = sort.key === key;
        if (active) th.setAttribute('aria-sort', sort.dir === 'asc' ? 'ascending' : 'descending');
        const id = incidentKey(incident) + ':' + key;
        const button = el('button', {type: 'button', class: 'sort-head', 'data-sort': id}, [
            el('span', {text: name}),
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
        const label = PSL.FINDING_TYPE_LABEL[finding.type] || finding.type || 'finding';
        const link = el('a', {
            href: PSL.ackRouteHash(finding.signature, incident.source_id),
            class: 'ack-link',
            'aria-label': 'Ack ' + label + ' on ' + (finding.source_endpoint || 'an unknown endpoint'),
            text: 'Ack'
        });
        link.addEventListener('click', function () {
            state.ackReturn = location.hash;
        });
        return link;
    }

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
                helpDot('A daemon with `[daemon.correlation]` on and `[daemon.hub_export]` set pushes each period a '
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
                proseInto(el('p'), 'A pair shows here once a daemon with `[daemon.correlation]` and '
                    + '`[daemon.hub_export]` on pushes an episode of it.')
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
            // Shown once: the next render of this panel, a refold or a re-read, asks again.
            delete state.incidentCorrelations[cacheKey];
            block.appendChild(el('p', {
                class: 'table-muted',
                text: 'The Hub did not return the correlations of this window.'
            }));
        } else if (held.rows.length > 0) {
            block.appendChild(el('div', {class: 'sources-wrap'}, [incidentEpisodesTable(held.rows)]));
        } else {
            block.appendChild(el('p', {
                class: 'empty-state',
                text: held.pushes
                    ? 'No correlation was active in this window.'
                    : (incident.source_name || incident.source_id) + ' pushes no correlation you can read, so none can show here.'
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
                el('span', {text: state.ackReturn ? 'Back to the incident' : 'Incidents'})
            ]),
            ruledOverline('// acknowledgment'),
            el('h1', {class: 'page-title', text: 'Acknowledge a finding'}),
            el('p', {
                class: 'page-sub',
                text: 'An ack lives in one daemon\'s own store, so it is taken source by source, and one '
                    + 'submit writes to every checked source, in the name this Hub knows you by.'
            })
        ])]);
        if (!route) {
            section.appendChild(el('div', {class: 'empty-state'}, [
                el('p', {class: 'empty-title', text: 'This link is incomplete.'}),
                el('p', {
                    text: 'It names no finding signature, or one the Hub refuses: longer than 1,024 characters, '
                        + 'or carrying a control character. Open it again from the dashboard that gave it.'
                })
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
                el('p', {text: 'The Hub could not read this finding or its sources. Reload the page.'})
            ]));
            return section;
        }
        if (!ack.finding) {
            section.appendChild(el('div', {class: 'empty-state'}, [
                el('p', {class: 'empty-title', text: 'The Hub holds no finding with this signature.'}),
                el('p', {}, [
                    el('span', {text: 'Retention removed it, or no source this Hub reads ever reported '}),
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
        const facts = [['Detections (7 days)', PSL.formatDetections(volume.detections, volume.detections_complete)]];
        if (volume.avoidable_io_ops != null) {
            facts.push(['Avoidable I/O (7 days)',
                PSL.formatDetections(volume.avoidable_io_ops, volume.detections_complete)]);
        }
        return facts;
    }

    /** One line per source with its count, and what keeps it short. */
    function ackDetectionLines(volume) {
        if (volume === undefined) {
            return [el('p', {class: 'field-note', text: 'The Hub could not read the detections of the last 7 days.'})];
        }
        if (volume === null) {
            return [el('p', {class: 'field-note', text: 'No detection known in the last 7 days'})];
        }
        return (volume.sources || []).map(function (source) {
            const gaps = (source.detections_gaps || []).map(PSL.gapLabel);
            const io = source.avoidable_io_ops == null ? ''
                : ', ' + PSL.formatDetections(source.avoidable_io_ops, source.detections_complete) + ' avoidable I/O';
            return el('p', {
                class: 'field-note',
                text: source.name + ': ' + PSL.formatDetections(source.detections, source.detections_complete) + io
                    + (gaps.length > 0 ? ' (' + gaps.join(', ') + ')' : '')
            });
        });
    }

    function ackFindingCard(envelope, volume) {
        const finding = envelope.finding || {};
        const template = (finding.pattern || {}).template;
        const facts = [
            ['service', finding.service || '?'],
            ['endpoint', finding.source_endpoint || '?'],
            ['first seen', PSL.dtHuman(envelope.first_seen)],
            ['last seen', PSL.dtHuman(envelope.last_seen)],
            // The API's enum, worded: not_observed reads "not observed".
            ['status', String(envelope.status || '?').replace(/_/g, ' ')]
        ].concat(ackDetectionFacts(volume));
        return el('div', {class: 'card params-panel'}, [
            el('div', {class: 'run-card-line'}, [
                el('span', {class: 'run-card-name', text: PSL.FINDING_TYPE_LABEL[finding.type] || finding.type || '?'}),
                el('span', {class: 'chip', 'data-sev': finding.severity || null, text: finding.severity || '?'})
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
        sentence.textContent = busy ? 'Writing to the daemons.' : plan.sentence;
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
            el('span', {text: 'Acknowledge'})
        ]);
        acknowledge.addEventListener('click', function () {
            submitAck('ack', rows);
        });
        const revoke = el('button', {type: 'button', class: 'pill-button', id: 'ack-revoke'}, [
            el('span', {text: 'Revoke'})
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
                        text: rows.length === 0
                            ? 'No source in this Hub carries this finding, so there is nothing to acknowledge from here.'
                            : 'This Hub cannot write an ack to any source that carries this finding. Each row says why.'
                    }),
                    rows.length === 0 ? null : el('div', {class: 'field sources-wrap'}, [ackSourcesTable(rows)])
                ]),
                daemonAckBlock(rows)
            ]);
        }
        return el('form', {class: 'card params-panel'}, [
            field('Reason', reason, 'required, kept by each daemon beside your name'),
            field('Expires', expiry, 'optional, the end of that day in UTC. Empty is a permanent ack'),
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
            head: '// acknowledge it on the daemon',
            sub: 'The daemon takes the ack itself, with its own key.',
            id: 'ack-command',
            spell: function (shellId) {
                return PSL.ackCommand(source, state.ack.signature, shellId);
            },
            copyLabel: 'Copy the ack command for ' + source.name,
            notes: [
                'Set `PERF_SENTINEL_DAEMON_API_KEY` to this daemon\'s ack key first, and write the reason '
                + 'in place of the last value. Add `--expires 7d` for an ack that lapses.',
                engineNote()
            ]
        });
    }

    function ackSourcesTable(rows) {
        const head = el('tr', {}, ['', 'Source', 'Current ack', 'What this page can do'].map(function (name) {
            return el('th', {text: name, scope: 'col'});
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
                        el('span', {class: 'chip chip-declared', text: row.environment || 'unknown'})
                    ]),
                    ackCell(row.ack),
                    el('td', {
                        id: reasonId,
                        text: row.note || (row.action === 'revoke' ? 'Revoke this ack.' : 'Acknowledge.')
                    })
                ]);
            }))
        ]);
    }

    /** `at` and `expires_at` are the daemon's own text, shown as it came. */
    function ackCell(ack) {
        if (!ack) return el('td', {}, [el('span', {class: 'table-muted', text: 'none known'})]);
        return el('td', {}, [
            el('span', {class: 'table-strong', text: ack.by || '?'}),
            el('span', {text: ' at ' + ack.at + (ack.expires_at ? ', until ' + ack.expires_at : ', no expiry')}),
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
                return {name: row.name, action: action, status: response.status, detail: payload && payload.detail};
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

    // ---------------------------------------------------- screen: disclosures

    const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September',
        'October', 'November', 'December'];
    const LEVEL_LABELS = {
        'none': 'no integrity',
        'hash-only': 'hash only',
        'signed': 'signed',
        'signed-with-attestation': 'signed + attestation',
        'audited': 'audited'
    };
    const RENDER_REASONS = {
        unsupported_engine: 'this Hub\'s engine cannot render disclosures yet',
        unsupported_schema_version: 'the report is newer than this Hub\'s engine',
        unsupported_characters: 'the report holds text the PDF fonts cannot draw',
        pdf_export_failed: 'the engine could not write the PDF',
        output_unwritable: 'the Hub could not write the PDF',
        timeout: 'the render took longer than the Hub allows',
        content_hash_mismatch: 'the engine found the report altered since its hash was baked',
        binding_mismatch: 'the engine found the attestation is not over this report'
    };
    const DISCLOSURE_FILES = [
        ['report.pdf', 'PDF/A-3b, carries the three files below as attachments', 'pdf'],
        ['report.json', 'the disclosure, hashed', 'report'],
        ['attestation.intoto.jsonl', 'in-toto v1 statement over report.json', 'attestation'],
        ['bundle.sig', 'Sigstore bundle, with the transparency log entry', 'bundle'],
        ['SHA256SUMS', 'one line per file, for sha256sum -c', 'report']
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

    function longDate(iso) {
        const parts = String(iso).split('-');
        return Number(parts[2]) + ' ' + MONTHS[Number(parts[1]) - 1] + ' ' + parts[0];
    }

    function periodDays(period) {
        return Math.round((Date.parse(period.to) - Date.parse(period.from)) / 86400000) + 1;
    }

    function periodLabel(period) {
        const parts = String(period.to).split('-');
        const month = Number(parts[1]);
        if (period.type === 'calendar-quarter') return parts[0] + ' Q' + (Math.floor((month - 1) / 3) + 1);
        if (period.type === 'calendar-year') return parts[0];
        if (period.type === 'calendar-month') return MONTHS[month - 1] + ' ' + parts[0];
        return longDate(period.from) + ' to ' + longDate(period.to);
    }

    function shortHash(hash) {
        const value = String(hash);
        return value.length > 20 ? value.slice(0, 11) + '…' + value.slice(-4) : value;
    }

    function checkGlyph() {
        return svg([['path', {d: 'M5 12.5l4.2 4.2L19 7'}]], 11);
    }

    /** What the report declares about itself, dashed: the Hub cannot check it. */
    function levelChip(level) {
        return el('span', {class: 'chip chip-declared disc-level', text: LEVEL_LABELS[level] || level || 'undeclared'});
    }

    /** What this Hub recomputed, solid. Anything it did not check reads as muted text, never a mark. */
    function checkChip(label, outcome) {
        if (outcome === 'checked') return el('span', {class: 'chip disc-checked'}, [el('span', {text: label}), checkGlyph()]);
        if (outcome === 'declared') return el('span', {
            class: 'chip chip-declared disc-level',
            text: label + ' declared'
        });
        return el('span', {
            class: 'chip disc-unchecked',
            text: outcome === 'absent' ? 'no ' + label : label + ' not checked'
        });
    }

    function disclosureChips(item) {
        const chips = [
            el('span', {class: 'chip', text: item.intent}),
            el('span', {class: 'chip', text: item.confidentiality}),
            levelChip(item.integrity_level)
        ];
        if (item.pdf === 'rendered') {
            chips.push(checkChip('hash', 'checked'));
            if (item.files.attestation) chips.push(checkChip('bound', 'checked'));
        } else {
            chips.push(el('span', {class: 'chip disc-unchecked', text: 'not checked here'}));
        }
        if (item.confidentiality === 'internal') chips.push(el('span', {
            class: 'chip chip-declared',
            text: 'never shared'
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
        return value == null ? 'n/a' : Number(value).toFixed(digits);
    }

    /** Runtime coverage on a track, with the official 75 % as a notch and the fill orange under it. */
    function coverageBar(coverage) {
        const notch = PSL.coverageNotch(coverage);
        if (!notch) return el('span', {class: 'disc-figure-sub', text: 'not reported'});
        const fill = el('span', {class: 'disc-track-fill'});
        fill.style.width = Math.min(100, notch.percent) + '%';
        return el('div', {
            class: 'disc-track', 'data-below': notch.below ? 'true' : null, role: 'img',
            'aria-label': notch.percent + ' % runtime coverage, ' + (notch.below ? 'under' : 'at or over')
                + ' the 75 % an official report needs'
        }, [
            fill,
            el('span', {class: 'disc-track-notch'}),
            el('span', {class: 'disc-track-mark', text: '75 % official'})
        ]);
    }

    function disclosureFigures(figures) {
        const f = figures || {};
        const coverage = PSL.coverageNotch(f.coverage);
        return el('div', {class: 'disc-figures'}, [
            figure('Energy', fixed(f.energy_kwh, 2), 'kWh', null),
            figure('Carbon, location-based', fixed(f.carbon_kgco2eq, 2), 'kgCO₂eq',
                f.carbon_kgco2eq == null ? null
                    : 'bracket ' + fixed(f.carbon_kgco2eq / 2, 2) + ' to ' + fixed(f.carbon_kgco2eq * 2, 2)),
            figure('Avoidable, canonical', f.avoidable_ratio == null ? 'n/a' : Math.round(f.avoidable_ratio * 100), '% of I/O',
                null),
            el('div', {class: 'disc-figure'}, [
                el('span', {class: 'disc-figure-label', text: 'Runtime coverage'}),
                el('span', {class: 'disc-figure-value'}, [
                    el('span', {text: coverage ? String(coverage.percent) : 'n/a'}),
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

    function disclosureFacts(item) {
        return el('div', {class: 'disc-facts'}, [
            fact('published', PSL.dur(Date.now() - item.published_at_ms) + ' ago'),
            item.signature_added_at_ms ? fact('signed copy', PSL.dur(Date.now() - item.signature_added_at_ms) + ' ago') : null,
            fact('pdf', item.pdf === 'rendered' ? 'rendered here' : 'not rendered'),
            el('span', {class: 'disc-fact disc-hash', title: item.content_hash, text: shortHash(item.content_hash)})
        ]);
    }

    function disclosureFileButton(id, name, primary, status) {
        const button = el('button', {type: 'button', class: 'pill-button pill-sm' + (primary ? ' pill-primary' : '')},
            [primary ? downloadGlyph() : null, el('span', {text: primary ? 'Download the PDF' : name})]);
        button.addEventListener('click', function () {
            button.disabled = true;
            PSL.downloadAs('/api/disclosures/' + id + '/files/' + name, name).then(function (saved) {
                button.disabled = false;
                if (status) status.textContent = saved ? 'Sent ' + name + ' to your browser.' : 'The Hub did not return ' + name + '.';
            });
        });
        return button;
    }

    function disclosureBanners(item) {
        const banners = [];
        if (item.withdrawn) {
            banners.push(el('div', {class: 'banner', 'data-tone': 'crit'}, [critGlyph(16), el('div', {
                text: 'Withdrawn ' + PSL.dur(Date.now() - item.withdrawn.at_ms) + ' ago by ' + (item.withdrawn.by || 'someone')
                    // A reason typed as a sentence brings its own full stop, and the banner adds one.
                    + ': ' + ((item.withdrawn.reason || '').replace(/\.+\s*$/, '') || 'no reason given')
                    + '. Its files stay, its share links are dead.'
            })]));
        } else if (item.intent === 'official' && (item.integrity_level === 'hash-only' || item.integrity_level === 'none')) {
            banners.push(el('div', {class: 'banner', 'data-tone': 'warn'}, [warningGlyph(16), el('div', {
                text: 'Official intent without a signature. Anyone can check this file was not altered since its hash '
                    + 'was baked, nobody can check who baked it. Sign the attestation with cosign and publish again to raise it.'
            })]));
        }
        if (item.pdf !== 'rendered') {
            // A render again the engine refused: the files are suspect, not merely unchecked.
            const refused = item.render_reason === 'content_hash_mismatch' || item.render_reason === 'binding_mismatch';
            const text = el('div', {
                text: (refused ? 'Refused here: ' : 'Not checked here: ')
                    + (RENDER_REASONS[item.render_reason] || item.render_reason || 'no render yet')
                    + '. The files are kept as they arrived, and the figures are the report\'s own.'
            });
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
            }, [el('span', {text: 'Details'})]),
            status
        ];
        return el('article', {class: 'card disc-card', 'data-withdrawn': item.withdrawn ? 'true' : null}, [
            el('header', {class: 'disc-card-head'}, [
                el('h2', {class: 'disc-card-title'}, [
                    el('a', {href: '#/disclosures/' + item.id, text: periodLabel(item.period)})
                ]),
                el('span', {
                    class: 'disc-dates',
                    text: longDate(item.period.from) + ' → ' + longDate(item.period.to) + ' · ' + periodDays(item.period) + ' days'
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
        return el('ol', {class: 'disc-rail', 'aria-label': 'Quarters'}, rail.map(function (cell) {
            const caption = cell.state === 'published' ? LEVEL_LABELS[cell.level] || cell.level
                : cell.state === 'internal' ? 'internal'
                    : cell.state === 'withdrawn' ? 'withdrawn'
                        : cell.state === 'current' ? 'closes in ' + cell.daysLeft + ' d' : 'not published';
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
            el('span', {class: 'disc-legend-label', text: 'declared'}),
            levelChip('hash-only'), levelChip('signed'), levelChip('signed-with-attestation'),
            el('span', {class: 'disc-legend-label', text: 'checked here'}),
            checkChip('hash', 'checked'), checkChip('bound', 'checked'),
            el('p', {
                class: 'disc-legend-note', text: 'A dashed level is what the report declares about itself. A solid '
                    + 'mark is what this Hub recomputed when it arrived: the content hash, and an attestation that names it. '
                    + 'The signature is checked on your machine, never here.'
            })
        ]);
    }

    function publishPanel() {
        return terminalBlock({
            head: '// publish the next one',
            sub: 'The last steps of the CI job that produces and signs the report.',
            text: PSL.publishCommand(location.origin),
            copyLabel: 'Copy the publish command',
            notes: ['Publishing takes the import key `Hub:Disclosures:ImportKey`, like a daemon\'s findings import. '
            + 'Reading this page needs `disclosures.read`, sharing and withdrawing need `disclosures.manage`. '
            + 'Neither is in the built-in reader or member role.']
        });
    }

    function renderDisclosuresScreen() {
        const section = el('section', {class: 'disc'}, [
            ruledOverline('// disclosures'),
            el('h1', {class: 'page-title', text: 'Energy and carbon, period by period'}),
            el('p', {
                class: 'page-sub', text: 'Each report was produced and signed outside the Hub by perf-sentinel '
                    + 'disclose, then published here by CI. The Hub recomputed its content hash on arrival and keeps the '
                    + 'files as they were sent. Public transparency, not regulatory-grade: every carbon figure carries a '
                    + '~2× bracket.'
            })
        ]);
        if (state.disclosures === null) {
            section.appendChild(el('div', {class: 'card skeleton', style: 'height:220px'}));
            return section;
        }
        if (state.disclosuresError) {
            section.appendChild(el('div', {class: 'banner', 'data-tone': 'crit'}, [
                critGlyph(16),
                el('div', {text: 'The Hub did not return its disclosures. Reload the page to ask again.'})
            ]));
            return section;
        }
        if (state.disclosures.length === 0) {
            section.appendChild(el('div', {class: 'empty-state'}, [
                el('p', {class: 'empty-title', text: 'No disclosure published yet.'}),
                el('p', {
                    text: 'A scheduled job builds one from the daemons\' archives and publishes it '
                        + 'with the import key, as below. It shows here once the Hub '
                        + 'has checked and kept it.'
                })
            ]));
            section.appendChild(publishPanel());
            return section;
        }
        section.appendChild(el('div', {class: 'disc-rail-wrap'}, [
            el('span', {class: 'disc-legend-label', text: 'periods'}),
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
            el('span', {class: 'disc-figure-label', text: 'Carbon with its ~2× bracket, log scale'}),
            el('div', {
                class: 'disc-bracket-track', role: 'img',
                'aria-label': fixed(carbon, 2) + ' kgCO₂eq, likely between ' + fixed(carbon / 2, 2) + ' and ' + fixed(carbon * 2, 2)
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
                el('span', {class: 'table-mono', text: Math.round(share * 100) + ' %'})])]);
        }
        const rows = applications.map(function (app) {
            return el('tr', {}, [
                el('td', {class: 'table-strong', text: app.display_name || app.service_name}),
                el('td', {
                    class: 'table-mono',
                    'data-align': 'right',
                    text: app.total_requests == null ? '' : Number(app.total_requests).toLocaleString('en')
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
                        text: 'in ' + host.partial.windows + ' of ' + host.partial.of + ' windows'
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
            rows.push(el('tr', {}, [
                el('td', {}, [el('span', {
                    class: 'table-strong',
                    text: (app && (app.service_name || app.name)) || String(app)
                }), document.createTextNode(' '),
                    el('span', {class: 'chip chip-declared', text: 'excluded'})]),
                el('td', {
                    class: 'table-muted',
                    colspan: '5',
                    text: (app && app.reason) || 'Declared by the operator, not measured.'
                })
            ]));
        });
        const declared = figures && figures.applications_declared;
        const measured = figures && figures.applications_measured;
        const coverage = declared != null && measured != null
            ? measured + ' of ' + declared + ' declared applications were measured.' : null;
        const hostNote = hosts.length === 0 ? null : 'The '
            + hosts.map(function (host) {
                return host.label.toLowerCase();
            }).join(' and ').replace('database hosts and broker hosts', 'database and broker hosts')
            + ' count in the total and in no application.';
        return detailPanel('// applications', [coverage, hostNote].filter(Boolean).join(' ') || null,
            [simpleTable(['Application', 'Requests', 'Energy share', 'kWh', 'kgCO₂eq', 'Anti-patterns'], rows,
                ['Requests', 'kWh', 'kgCO₂eq', 'Anti-patterns'])]);
    }

    function integrityPanel(detail) {
        const rows = PSL.checkRows(detail).map(function (row) {
            const outcome = row.outcome === 'checked' ? checkChip('checked', 'checked')
                : row.outcome === 'declared' ? el('span', {class: 'chip chip-declared disc-level', text: 'declared'})
                    : el('span', {
                        class: 'chip disc-unchecked',
                        text: row.outcome === 'not_checked' ? 'not checked' : row.outcome
                    });
            return el('tr', {}, [
                el('td', {class: 'table-strong', text: row.claim}),
                el('td', {class: 'table-mono', text: row.by}),
                el('td', {}, [outcome])
            ]);
        });
        const item = detail.disclosure;
        return detailPanel('// integrity',
            'Who checked what. The Hub recomputes a hash with its engine. It cannot check a signature, which is only '
            + 'worth what the verifier trusts.',
            [
                simpleTable(['Claim', 'Checked by', 'Outcome'], rows),
                el('dl', {class: 'disc-defs'}, [
                    el('dt', {text: 'content_hash'}), el('dd', {class: 'table-mono', text: item.content_hash}),
                    el('dt', {text: 'report_uuid'}), el('dd', {class: 'table-mono', text: item.report_uuid}),
                    el('dt', {text: 'expected signer'}), el('dd', {
                        class: 'table-mono',
                        text: detail.expected_identity ? detail.expected_identity + ' · ' + detail.expected_issuer
                            : 'none in this Hub\'s configuration'
                    })
                ]),
                terminalBlock({
                    head: '// verify on your machine',
                    sub: 'Download the PDF, then run the engine\'s own verifier on it.',
                    text: PSL.verifyCommand(detail),
                    copyLabel: 'Copy the verify command',
                    notes: ['TRUSTED needs a signed report and an expected signer. The expected signer comes from this '
                    + 'Hub\'s configuration, never from the report.']
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
                el('td', {class: 'table-mono', text: file[1]}),
                el('td', {'data-align': 'right'}, [disclosureFileButton(item.id, file[0], false, status)])
            ]);
        });
        return detailPanel('// files', 'Exactly what was published, kept in the Hub\'s database so its backup carries '
            + 'them. The PDF is the one file this Hub produced, with the engine it embeds.',
            [simpleTable(['File', 'Role', ''], rows), status]);
    }

    function linkState(link) {
        return link.state === 'active' ? 'expires in ' + PSL.dur(link.expires_at_ms - Date.now())
            : link.state === 'revoked' ? 'revoked by ' + link.revoked_by
                : link.state === 'withdrawn' ? 'dead with the withdrawal' : 'expired';
    }

    function shareLinksPanel(detail, manage) {
        const item = detail.disclosure;
        const rows = detail.share_links.map(function (link) {
            const revoke = manage && link.state === 'active'
                ? disclosureActionButton('Revoke', function () {
                    return sendDisclosure('/api/disclosures/' + item.id + '/share-links/' + link.id, 'DELETE');
                }, 'Link revoked.')
                : null;
            return el('tr', {}, [
                el('td', {class: 'table-mono', text: link.created_by}),
                el('td', {class: 'table-mono', text: PSL.dur(Date.now() - link.created_at_ms) + ' ago'}),
                el('td', {class: 'table-mono', text: linkState(link)}),
                el('td', {class: 'table-mono', 'data-align': 'right', text: String(link.downloads)}),
                el('td', {
                    class: 'table-mono',
                    text: link.last_used_at_ms ? PSL.dur(Date.now() - link.last_used_at_ms) + ' ago' : 'never'
                }),
                el('td', {'data-align': 'right'}, [revoke])
            ]);
        });
        const children = [];
        if (state.disclosureLink && state.disclosureLink.id === item.id) children.push(terminalBlock({
            head: '// new share link',
            sub: 'Shown this once. Send the PDF\'s address. verify-hash --url works on the report.json beside it.',
            text: state.disclosureLink.pdfUrl + '\n\nperf-sentinel verify-hash --url ' + state.disclosureLink.reportUrl,
            copyLabel: 'Copy the share link'
        }));
        children.push(rows.length
            ? simpleTable(['Created by', 'Created', 'State', 'Uses', 'Last use', ''], rows, ['Uses'])
            : el('p', {
                class: 'sources-note', text: item.confidentiality === 'internal'
                    ? 'An internal report is never shared.' : 'No share link yet.'
            }));
        return detailPanel('// share links', 'A share link serves this disclosure\'s PDF and its three files under their '
            + 'fixed names to whoever holds it, with no session. It expires within 30 days and counts its uses.', children);
    }

    function deliveriesPanel(detail, manage) {
        if (detail.deliveries.length === 0) return null;
        const item = detail.disclosure;
        const rows = detail.deliveries.map(function (delivery) {
            const retry = manage && delivery.state !== 'delivered'
                ? disclosureActionButton('Retry', function () {
                    return sendDisclosure('/api/disclosures/' + item.id + '/deliveries/' + delivery.id + '/retry', 'POST');
                }, 'Delivery queued again.')
                : null;
            return el('tr', {}, [
                el('td', {class: 'table-strong', text: delivery.webhook}),
                el('td', {class: 'table-mono', text: delivery.event}),
                el('td', {
                    class: 'table-mono',
                    text: delivery.last_attempt_ms ? PSL.dur(Date.now() - delivery.last_attempt_ms) + ' ago' : 'not yet'
                }),
                el('td', {}, [el('span', {
                    class: 'status-pill', 'data-status': delivery.state === 'delivered' ? 'succeeded'
                        : delivery.state === 'failed' ? 'failed' : 'queued',
                    text: delivery.state + (delivery.last_status ? ' · ' + delivery.last_status : '')
                        + ' · ' + delivery.attempts + ' tries'
                })]),
                el('td', {'data-align': 'right'}, [retry])
            ]);
        });
        return detailPanel('// deliveries', 'Webhooks declared in the Hub\'s configuration, one signed POST each at '
            + 'publication and at withdrawal. The Hub shows their names, never their URLs.',
            [simpleTable(['Webhook', 'Event', 'Last attempt', 'Outcome', ''], rows)]);
    }

    function methodPanel(report) {
        if (!report) return null;
        const methodology = report.methodology || {};
        const disclaimers = report.notes && Array.isArray(report.notes.disclaimers) ? report.notes.disclaimers : [];
        const defs = [
            ['specification', methodology.sci_specification],
            ['conformance', methodology.conformance],
            ['patterns', Array.isArray(methodology.enabled_patterns) ? methodology.enabled_patterns.join(', ') : null],
            ['carbon intensity', methodology.calibration_inputs && methodology.calibration_inputs.carbon_intensity_source]
        ].filter(function (pair) {
            return pair[1];
        });
        return detailPanel('// method and limits', 'As the report states them. The disclaimers are copied verbatim.', [
            el('dl', {class: 'disc-defs'}, defs.flatMap(function (pair) {
                return [el('dt', {text: pair[0]}), el('dd', {class: 'table-mono', text: String(pair[1])})];
            })),
            disclaimers.length ? el('ol', {class: 'disc-disclaimers'}, disclaimers.map(function (text) {
                return el('li', {text: String(text)});
            })) : null
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
                    : (outcome && outcome.payload && (outcome.payload.detail || outcome.payload.reason))
                    || 'The Hub refused this (' + (outcome ? outcome.status : 0) + ').';
                loadDisclosure(PSL.readDisclosureId(location.hash));
            });
        });
        return button;
    }

    function withdrawForm(item) {
        const reason = el('textarea', {
            class: 'disc-reason', rows: '2', maxlength: '500', 'aria-label': 'Reason for the withdrawal',
            placeholder: 'Why, in a sentence: it is kept and sent with the withdrawal.'
        });
        const submit = disclosureActionButton('Withdraw this disclosure', function () {
            if (!reason.value.trim()) return Promise.resolve({
                ok: false,
                status: 400,
                payload: {detail: 'Give a reason first.'}
            });
            return sendDisclosure('/api/disclosures/' + item.id + '/withdraw', 'POST', {reason: reason.value.trim()});
        }, 'Withdrawn. Its share links are dead, and its webhooks are told.');
        return el('div', {class: 'disc-withdraw'}, [
            el('p', {
                text: 'Withdrawing is final. The files stay, every share link dies, and each webhook told of the '
                    + 'publication is told of the withdrawal. Publish a corrected report under a new uuid.'
            }),
            reason,
            el('div', {class: 'disc-actions'}, [submit])
        ]);
    }

    function detailActions(detail, manage) {
        const item = detail.disclosure;
        const status = el('span', {class: 'terminal-status', role: 'status'});
        const buttons = [item.files.pdf ? disclosureFileButton(item.id, 'report.pdf', true, status) : null];
        if (manage && !item.withdrawn && item.confidentiality !== 'internal') {
            buttons.push(disclosureActionButton('New share link', function () {
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
            }, 'Share link created. Copy it below: it is not shown again.'));
        }
        if (manage && item.pdf !== 'rendered') {
            buttons.push(disclosureActionButton('Render again', function () {
                return sendDisclosure('/api/disclosures/' + item.id + '/render', 'POST', {});
            }, 'Rendered again.'));
        }
        if (manage && !item.withdrawn) {
            const toggle = el('button', {
                type: 'button', class: 'pill-button pill-sm',
                'aria-expanded': state.withdrawOpen ? 'true' : 'false', text: 'Withdraw'
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
        }, [el('span', {text: 'Back to disclosures'})]);
        const section = el('section', {class: 'disc'}, [back]);
        if (state.disclosureError) {
            section.appendChild(el('div', {class: 'banner', 'data-tone': 'crit'}, [critGlyph(16),
                el('div', {text: 'The Hub did not return this disclosure. It may not exist, or you may not read disclosures.'})]));
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
        [
            ruledOverline('// disclosure · ' + periodLabel(item.period)),
            el('h1', {
                class: 'page-title', text: item.organisation + ', ' + longDate(item.period.from) + ' to '
                    + longDate(item.period.to)
            }),
            el('p', {
                class: 'page-sub', text: item.intent.charAt(0).toUpperCase() + item.intent.slice(1) + ' intent, '
                    + item.confidentiality + ' confidentiality. Published to this Hub ' + PSL.dur(Date.now() - item.published_at_ms)
                    + ' ago' + (item.pdf === 'rendered'
                        ? ', its content hash recomputed on arrival and its bytes kept as they came.'
                        : ', kept as it came without a check here.')
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
        section.appendChild(detailPanel('// at a glance', null, [
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
            ruledOverline('// access'),
            el('h1', {class: 'page-title', text: 'Your access'}),
            el('p', {
                class: 'page-sub',
                text: 'Rights are declared in the Hub\'s configuration, not on this page. Its one action '
                    + 'revokes a terminal token you approved.'
            })
        ]);
        if (banner) section.appendChild(noGrantBanner(banner));

        section.appendChild(accessCard('signed in as', [
            el('p', {text: signedIn.line}),
            signedIn.note ? el('p', {class: 'access-muted', text: signedIn.note}) : null,
            el('p', {}, [
                el('span', {text: signedIn.subjectLabel}),
                el('code', {class: 'access-subject', text: signedIn.subject})
            ])
        ]));
        section.appendChild(accessCard('your groups', [
            access.groups.length === 0
                ? el('p', {class: 'access-muted', text: 'No group.'})
                : el('ul', {class: 'access-chips', 'aria-label': 'Your groups'}, access.groups.map(function (group) {
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
                head: '// send this to an administrator',
                sub: 'Your name, the id an administrator lists, and the groups a grant can name.',
                text: PSL.accessRequestText(access),
                copyLabel: 'Copy the access request'
            }));
        } else {
            const reach = PSL.accessReachRows(access);
            section.appendChild(accessCard('what you reach', [
                reach.length === 0
                    ? el('p', {text: PSL.ACCESS_NO_REACH})
                    : accessTable('What your grants give you', ['Project', 'Group', 'Role', 'Permissions'],
                        reach.map(function (row) {
                            return [row.project, row.group, row.role, row.permissions];
                        }))
            ]));
        }
        if (state.tokens || state.tokensFailed) {
            section.appendChild(tokensCard((state.tokens || []).filter(function (token) {
                return token.subject === access.subject;
            }), 'your terminal tokens', 'tokens-own'));
        }
        if (access.admin) {
            section.appendChild(el('p', {class: 'access-pointer'}, [
                el('span', {text: 'You administer this Hub. '}),
                el('a', {href: '#/coverage', text: 'Coverage'}),
                el('span', {text: ' lists what no project covers, everyone\'s terminal tokens and the whole configuration.'})
            ]));
        } else if (access.projects.length > 0) {
            section.appendChild(accessCard('projects you reach', [projectsTable(access.projects)]));
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
            ruledOverline('// coverage'),
            el('h1', {class: 'page-title', text: 'What access control covers'}),
            el('p', {
                class: 'page-sub',
                text: 'Findings no project covers are read only by grants on "*". Rights change in the Helm '
                    + 'values hub.access and apply at the next restart.'
            })
        ]);
        if (access.unmatched) section.appendChild(attentionCard(access.unmatched));
        const snippet = access.unmatched ? PSL.coverageSnippet(access.unmatched) : null;
        if (snippet) {
            section.appendChild(terminalBlock({
                head: '// patterns to paste',
                sub: 'One line per uncovered service, attribute and source.',
                text: snippet,
                copyLabel: 'Copy the patterns'
            }));
        }
        if (access.conflicts) section.appendChild(conflictsCard(access.conflicts));
        if (state.tokens || state.tokensFailed) {
            section.appendChild(tokensCard(state.tokens || [], 'terminal tokens, everyone\'s', 'tokens-all'));
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
            el('p', {
                text: 'perf-sentinel login gets a terminal token once its code is approved. It carries the '
                    + 'approver\'s access, never admin, until the session that approved it ends.'
            }),
            el('div', {role: 'status', class: 'access-status'}, notice ? [el('div', {
                class: 'banner',
                'data-tone': notice.ok ? 'ok' : 'crit'
            }, [el('div', {text: notice.text})])] : []),
            state.tokensFailed
                ? el('div', {class: 'banner', 'data-tone': 'crit'}, [
                    el('div', {text: 'The Hub did not list the terminal tokens. Reload the page to try again.'})
                ])
                : rows.length === 0
                    ? el('p', {class: 'access-muted', text: 'No terminal token.'})
                    : accessTable('Terminal tokens', ['Owner', 'Requested from', 'Created', 'Expires', 'Last used', ''],
                        rows.map(function (row) {
                            return [row.owner, row.from, row.created, row.expires, row.lastUsed, revokeButton(row, headingId)];
                        }))
        ], headingId);
    }

    function revokeButton(row, card) {
        const label = 'Revoke the token of ' + row.owner + ' created ' + row.created;
        const button = el('button', {type: 'button', class: 'pill-button', text: 'Revoke', 'aria-label': label});
        let armed = false;
        const disarm = function () {
            armed = false;
            button.textContent = 'Revoke';
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
                button.textContent = 'Confirm revoke';
                button.setAttribute('aria-label', 'Confirm: ' + label + '. Its terminal stops working.');
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
        return accessTable('Projects and their patterns', ['Project', 'Patterns'], projects.map(function (project) {
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
            count === 0 ? el('p', {class: 'access-muted', text: 'None declared.'}) : body
        ]);
    }

    function configurationCard(configuration) {
        const now = Date.now();
        return accessCard('configuration', [
            el('p', {class: 'access-muted', text: 'As the Hub loaded it from hub.access at its last start.'}),
            configurationPart('Projects', configuration.projects.length, projectsTable(configuration.projects)),
            configurationPart('Roles', configuration.roles.length,
                accessTable('Roles', ['Role', 'Permissions'], configuration.roles.map(function (role) {
                    return [role.built_in ? role.name + ' (built-in)' : role.name, role.permissions.join(', ')];
                }))),
            configurationPart('Grants', configuration.grants.length,
                accessTable('Grants', ['Group', 'Project', 'Role'], configuration.grants.map(function (grant) {
                    return [grant.group, grant.project, grant.role];
                }))),
            configurationPart('Internal groups', configuration.internal_groups.length,
                accessTable('Internal groups', ['Internal group', 'Members'], configuration.internal_groups.map(function (group) {
                    return ['internal:' + group.name, group.members.length === 0 ? 'no member' : group.members];
                }))),
            configurationPart('Admins', configuration.admins.length + configuration.admin_groups.length,
                el('dl', {class: 'access-pairs'}, [
                    el('dt', {text: 'By id'}),
                    el('dd', {text: configuration.admins.length === 0 ? 'none' : configuration.admins.join(', ')}),
                    el('dt', {text: 'By group'}),
                    el('dd', {text: configuration.admin_groups.length === 0 ? 'none' : configuration.admin_groups.join(', ')})
                ])),
            configurationPart('Machine tokens', configuration.machine_tokens.length,
                accessTable('Machine tokens', ['Machine token', 'Group', 'Expires'], configuration.machine_tokens.map(function (token) {
                    return [token.name, token.group, PSL.accessExpiry(token.expires_at_ms, now)];
                }))),
            configurationPart('Local accounts', configuration.local_accounts.length,
                accessTable('Local accounts', ['Local account', 'Signs in as'], configuration.local_accounts.map(function (name) {
                    return [name, 'local:' + name];
                })))
        ]);
    }

    /** What no project covers, the one part of the screen that asks for a change. */
    function attentionCard(unmatched) {
        const count = unmatched.services.length + unmatched.attributes.length + unmatched.sources.length;
        if (count === 0) {
            return accessCard('needs attention', [
                el('p', {text: 'Every finding the Hub holds belongs to a project. Nothing to assign.'})
            ]);
        }
        const parts = [el('p', {
            text: count + (count === 1 ? ' entry is' : ' entries are') + ' read by no project. Add a pattern that '
                + 'names it to the project of the team that owns it.'
        })];
        if (unmatched.services.length > 0) {
            parts.push(accessTable('Services no project covers', ['Service', 'Seen at', 'Findings'],
                unmatched.services.map(function (row) {
                    return [row.service == null ? '(no service)' : row.service,
                        row.source_ids.map(sourceName).join(', '), String(row.findings)];
                })));
        }
        if (unmatched.attributes.length > 0) {
            parts.push(accessTable('Attributes no project covers', ['Attribute', 'Findings'],
                unmatched.attributes.map(function (row) {
                    return [row.key + '=' + row.value, String(row.findings)];
                })));
        }
        if (unmatched.sources.length > 0) {
            parts.push(accessTable('Sources no project names', ['Source no project names', 'Findings'],
                unmatched.sources.map(function (row) {
                    // A trace backend stores no finding of its own: its runs are read live.
                    return [sourceName(row.source_id), row.findings == null ? 'none stored, trace backend' : String(row.findings)];
                })));
        }
        if (unmatched.truncated) {
            parts.push(el('p', {class: 'access-muted', text: 'Each list stops at its first 200 entries.'}));
        }
        return accessCard('needs attention', parts);
    }

    function conflictsCard(conflicts) {
        return accessCard('deployments that disagree on a project attribute', [
            el('p', {
                text: 'A project\'s attribute pattern reads these copies but cannot write them: the copy carries two '
                    + 'values of the key, or was also seen under a grouping without it. An acknowledgment through an '
                    + 'attribute grant is refused on them. A source or service grant for the team that owns the service '
                    + 'can acknowledge them.'
            }),
            conflicts.copies.length === 0
                ? el('p', {class: 'access-muted', text: 'None.'})
                : accessTable('Findings whose deployments disagree',
                    ['Source', 'Signature', 'Service', 'Key', 'Values', 'Groupings missing the key'],
                    conflicts.copies.map(function (row) {
                        return [sourceName(row.source_id), row.signature, row.service || '(no service)', row.key,
                            row.values.join(', '), String(row.identities_without_key)];
                    })),
            conflicts.truncated ? el('p', {class: 'access-muted', text: 'Only the first 200 are listed.'}) : null
        ]);
    }

    function renderRecentScreen() {
        const section = el('section', {}, [
            ruledOverline('// recent analyses'),
            el('h1', {class: 'page-title', text: 'Analyses this Hub still holds'}),
            el('p', {
                class: 'page-sub',
                text: 'The team\'s short memory, not an audit trail: reports are deleted '
                    + state.status.limits.report_retention_hours + ' hours after they succeed, so a link shared '
                    + 'yesterday is already dead.'
            })
        ]);

        if (!state.runs) {
            section.appendChild(el('div', {class: 'card skeleton', style: 'height:120px;margin-top:18px'}));
            return section;
        }
        if (state.runs.length === 0) {
            section.appendChild(el('div', {class: 'empty-state'}, [
                el('p', {
                    class: 'empty-title', text: 'No analysis in the last '
                        + state.status.limits.report_retention_hours + ' hours.'
                }),
                el('p', {
                    text: 'Retention empties this list after '
                        + state.status.limits.report_retention_hours + ' idle hours.'
                }),
                el('div', {class: 'outcome-actions empty-actions'}, [
                    el('a', {class: 'action action-filled', href: '#/new', text: 'Run an analysis'})
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
                el('p', {
                    text: 'These analyses were produced by ' + binaries.join(' and ') + '. Counts from '
                        + binaries.length + ' binaries are not directly comparable: a detector added between '
                        + 'minors changes what gets found, not only how much. The label on each card names which '
                        + 'binary did the detecting.'
                })
            ]));
        }

        const tuned = state.runs.filter(function (run) {
            return Object.keys((run.request || {}).detection || {}).length > 0;
        });
        if (tuned.length > 0 && tuned.length < state.runs.length) {
            section.appendChild(el('div', {class: 'banner', 'data-tone': 'warn'}, [
                warningGlyph(16),
                el('p', {
                    text: tuned.length + (tuned.length === 1 ? ' run' : ' runs') + ' here changed the '
                        + 'detection thresholds. Their counts are not comparable with the rest: a threshold '
                        + 'decides what gets reported, so a lower count can mean a quieter service or simply '
                        + 'a detector that was told to look for less. Each card names the thresholds it used.'
                })
            ]));
        }

        section.appendChild(el('div', {class: 'run-list'}, state.runs.map(runCard)));
        return section;
    }

    function runCard(run) {
        const key = PSL.statusKey(run);
        const card = el('a', {class: 'run-card', 'data-status': key, href: '#/run/' + run.id});

        card.appendChild(el('span', {class: 'run-card-line'}, [
            el('span', {class: 'status-pill', 'data-status': key, text: key === 'empty' ? 'succeeded · empty' : key}),
            el('span', {class: 'run-card-name', text: run.source_name}),
            el('span', {class: 'chip', text: PSL.KIND_LABEL[run.kind] || run.kind}),
            el('span', {
                class: 'chip chip-declared',
                text: run.environment,
                title: 'Declared by the source\'s configuration, not measured.'
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
            : key === 'queued' ? 'not started' : PSL.dur(now - started) + ' so far';
        const facts = [['by', run.requested_by], ['ran', ran]];
        if (run.producer_version) facts.push([PSL.detector(run.kind), run.producer_version,
            PSL.skew(run.producer_version) ? 'warn' : 'mono']);
        facts.push(['started', PSL.dur(now - started) + ' ago']);
        facts.push(['expires', run.expires_at_ms ? expiryText(run) : 'n/a',
            run.expires_at_ms && run.expires_at_ms < now ? 'crit' : 'mono',
            run.expires_at_ms ? function () {
                return expiryText(run);
            } : null]);
        const tuned = Object.keys((run.request || {}).detection || {});
        if (tuned.length > 0) {
            facts.push(['thresholds', tuned.length === 1 ? '1 changed' : tuned.length + ' changed', 'warn']);
        }
        if (run.error_code) facts.push(['error', run.error_code, 'crit']);
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
                text: access ? 'No access page on this Hub.' : 'Nothing lives at this address.'
            }),
            el('p', {
                class: 'page-sub',
                text: access
                    ? 'Access control is off here, or the Hub could not read who you are, so this page has nothing to show.'
                    : 'The Hub has no screen called "' + route + '". The link may be mistyped, or older than this Hub.'
            }),
            el('div', {class: 'outcome-actions'}, [
                el('a', {class: 'action action-filled', href: '#/new', text: 'Run an analysis'}),
                el('a', {class: 'action action-secondary', href: '#/recent', text: 'All analyses'})
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
            el('span', {class: 'report-loading-text', text: 'Loading the report'})
        ]);
        if (!run) {
            stage.appendChild(loading);
            return shell;
        }
        if (key !== 'succeeded' && key !== 'empty') {
            stage.appendChild(reportGone(run, key));
            return shell;
        }
        const frame = el('iframe', {class: 'report-frame', src: '/reports/' + id + '.html', title: 'Analysis report'});
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
                el('span', {text: 'Back to the run'})
            ]) : null,
            el('span', {class: 'report-path', text: 'report / ' + id}),
            el('span', {class: 'report-spacer'}),
            lifetime
        ]);
    }

    /** Why a report link opens on nothing, and the way on from there. */
    function reportGone(run, key) {
        const view = {
            queued: ['The report is not ready yet.', 'The analysis is still waiting for a worker. Its page says when the report is ready.'],
            running: ['The report is not ready yet.', 'The analysis is still reading ' + run.source_name + '. Its page says when the report is ready.'],
            failed: ['This analysis wrote no report.', 'It failed before the engine produced one. Its page says why.'],
            interrupted: ['This analysis wrote no report.', 'It was interrupted before the engine produced one.'],
            expired: ['This report is gone.', 'Its retention ended and the Hub deleted it. Running the same analysis '
            + 'again writes a new report from what ' + run.source_name + ' holds now.']
        }[key] || ['This report is gone.', 'The Hub no longer serves it.'];
        const finished = key !== 'queued' && key !== 'running';
        return el('div', {class: 'gone report-gone'}, [
            el('h1', {class: 'page-title', text: view[0]}),
            el('p', {class: 'page-sub', text: view[1]}),
            el('div', {class: 'outcome-actions'}, [
                finished ? actionButton(rerunAction('Run it again with the same parameters', true), true) : null,
                el('a', {class: 'action action-secondary', href: '#/run/' + run.id, text: 'Open the run'})
            ]),
            finished ? el('p', {class: 'outcome-note', text: PSL.argsLine(run)}) : null
        ]);
    }

    function reportLifetime(id) {
        const run = state.run && state.run.id === id ? state.run : null;
        const version = state.status && state.status.engine_version;
        // No run, no report: crediting an engine with it would describe a page that does not exist.
        if (!run) return '';
        const rendered = 'Rendered by perf-sentinel ' + (version || 'unknown');
        if (!run.expires_at_ms) return rendered;
        const left = run.expires_at_ms - Date.now();
        return rendered + (left > 0 ? ' · expires in ' + PSL.durPrecise(left) : ' · expired');
    }

    // ------------------------------------------------------------------ boot

    // A Grafana link arrives as `/?ack=<signature>`, with the environment and the
    // source it was opened from when it has them. It becomes the hash route
    // before anything reads the route, and the query leaves the address bar.
    const ackEntry = PSL.ackEntryHash(location.search);
    if (ackEntry) history.replaceState(null, '', location.pathname + ackEntry);
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
