/**
 * perf-sentinel Hub launcher — the guide that walks a screen.
 *
 * Classic script, loaded after launcher.js and before app.js. It reads `PSL` and never app.js:
 * app.js hands `start` a source of steps and calls `refresh` after each render, since a render
 * replaces `#main` and every element a step points at. The guide finds them again by their
 * `data-guide` attribute. It reads only what the screen already loaded and sends no request. It stores
 * one thing, the role last chosen under I am…, in this browser, to tick it in the menu next time.
 */
(function (global) {
    'use strict';

    const PSL = global.PSL;
    const PAD = 7;
    const GAP = 18;
    const EDGE = 16;
    const SHELL = ['.shell-topbar', '#main', '.shell-footer'];
    // Live lists, made once: placeMarks runs on every scroll frame.
    const NARROW = matchMedia('(max-width: 760px)');
    const STILL = matchMedia('(prefers-reduced-motion: reduce)');
    const ROLE_KEY = 'perf-sentinel:guide-role';

    /** @type {{source: () => import('../types').GuideSource, guide: string, status: string, variant: string,
     *  id: string | null, at: number,
     *  steps: import('../types').GuideStep[], all: import('../types').GuideStep[], frame: number, from: number,
     *  painted: string, path: {stops: import('../types').GuidePathStop[], skipped: string[], at: number} | null,
     *  heading: string | null} | null} */
    let open = null;
    /** @type {{root: HTMLElement, hole: HTMLElement, marks: HTMLElement, card: HTMLElement, title: HTMLElement,
     *  body: HTMLElement, count: HTMLElement, quit: HTMLButtonElement, prev: HTMLButtonElement,
     *  next: HTMLButtonElement} | null} */
    let dom = null;

    /**
     * @template {keyof HTMLElementTagNameMap} K
     * @param {K} tag
     * @param {{class?: string}} [attrs]
     * @param {string} [text]
     * @returns {HTMLElementTagNameMap[K]}
     */
    function node(tag, attrs, text) {
        const made = document.createElement(tag);
        if (attrs && attrs.class) made.className = attrs.class;
        if (text !== undefined) made.textContent = text;
        return made;
    }

    /**
     * The elements found so far: a render detaches the old ones, which is when they are looked up again.
     * @type {Map<string, Element>}
     */
    const cache = new Map();

    /** @param {string} name */
    function anchor(name) {
        const kept = cache.get(name);
        // A node can lose its name without leaving the page: a fold that closes unnames its detail.
        if (kept && kept.isConnected && kept.getAttribute('data-guide') === name) return kept;
        const node = document.querySelector('[data-guide="' + name + '"]');
        if (node) cache.set(name, node);
        else cache.delete(name);
        return node;
    }

    /**
     * A step is shown when one of its elements is on the page; one with none is a centred card.
     * @param {import('../types').GuideStep} step
     */
    function present(step) {
        return step.anchors.length === 0 || step.anchors.some(anchor);
    }

    function reducedMotion() {
        return STILL.matches;
    }

    function build() {
        const root = node('div', {class: 'guide-root'});
        root.id = 'guide-root';
        const hole = node('div', {class: 'guide-hole'});
        const marks = node('div', {class: 'guide-marks'});
        marks.setAttribute('aria-hidden', 'true');
        const card = node('div', {class: 'guide-card'});
        card.setAttribute('role', 'dialog');
        card.setAttribute('aria-modal', 'true');
        card.setAttribute('aria-labelledby', 'guide-title');
        // The live region only speaks changes, so the first card's text is read from here on focus.
        card.setAttribute('aria-describedby', 'guide-body');
        card.tabIndex = -1;
        const title = node('h2', {class: 'guide-title'});
        title.id = 'guide-title';
        const body = node('div', {class: 'guide-body'});
        body.id = 'guide-body';
        const count = node('span', {class: 'guide-count'});
        const quit = node('button', {class: 'guide-quit'});
        const prev = node('button', {class: 'pill-button'});
        const next = node('button', {class: 'pill-button pill-primary'});
        [quit, prev, next].forEach(function (button) {
            button.type = 'button';
        });
        quit.addEventListener('click', stop);
        prev.addEventListener('click', function () {
            move(-1);
        });
        next.addEventListener('click', function () {
            if (open.status === 'ready' && open.at < open.steps.length - 1) move(1);
            else if (moreStops()) enter(open.path.at + 1);
            else stop();
        });
        const nav = node('div', {class: 'guide-nav'});
        nav.append(count, prev, next);
        // One live region for the title, the text and the count, so a step is read out whole.
        const live = node('div');
        live.setAttribute('aria-live', 'polite');
        live.append(title, body, nav);
        card.append(quit, live);
        root.append(hole, marks, card);
        document.body.appendChild(root);
        return {root, hole, marks, card, title, body, count, quit, prev, next};
    }

    /**
     * Reads the source again and keeps the step on screen by its id; one gone gives way to the next
     * that is still there.
     */
    function read() {
        // On its way to a path's next screen the guide reads nothing until the route is drawn. The hash
        // changes at once and the screen on the hashchange after it: a render landing in between would
        // read the stop on the old screen, so arrived() waits for that event.
        if (open.heading !== null) {
            open.status = 'wait';
            return false;
        }
        const given = open.source();
        const was = open.steps[open.at];
        const lastId = was ? was.id : open.id;
        open.status = given.status;
        open.variant = given.variant || '';
        if (given.status !== 'ready') return false;
        const all = given.steps;
        const steps = all.filter(present);
        // A source with nothing to show yet is waited on, so a later guide can reuse the controller.
        if (!steps.length) {
            open.status = 'wait';
            return false;
        }
        let from = all.findIndex(function (step) {
            return step.id === lastId;
        });
        // A step dropped from the list altogether resumes where it stood in it.
        if (from < 0) from = lastId === null ? 0 : open.from;
        // The last step has no element, so one is always there.
        const kept = all.slice(from).find(present) || steps[steps.length - 1];
        open.steps = steps;
        open.all = all;
        open.at = steps.indexOf(kept);
        open.from = all.indexOf(kept);
        open.id = kept.id;
        return kept.id !== lastId;
    }

    /**
     * A live region reads out a node it rewrites, even with the same text.
     * @param {HTMLElement} button
     * @param {string} text
     */
    function say(button, text) {
        if (button.textContent !== text) button.textContent = text;
    }

    /** Whether a path runs and has a screen after this one. */
    function moreStops() {
        return Boolean(open.path) && open.path.at < open.path.stops.length - 1;
    }

    /** The sentence naming the screens a path skipped, for its very last card, or null. */
    function skippedText() {
        if (!open.path || moreStops() || !open.path.skipped.length) return null;
        const screens = open.path.skipped.map(function (screen) {
            return PSL.t('shell.tab.' + screen);
        }).join(', ');
        return PSL.t(open.path.skipped.length === 1 ? 'guide.path.skipped.one' : 'guide.path.skipped.other', {screens});
    }

    /** @param {import('../types').GuideStep} step */
    function paintStep(step) {
        dom.title.textContent = step.title;
        /** @type {HTMLElement[]} */
        const parts = step.body.map(function (text) {
            return node('p', {}, text);
        });
        const marks = (step.marks || []).filter(function (mark) {
            return anchor(mark.anchor);
        });
        if (marks.length) {
            const legend = node('ol', {class: 'guide-legend'});
            marks.forEach(function (mark, index) {
                const item = node('li');
                item.append(node('span', {class: 'guide-num'}, String(index + 1)), node('span', {}, mark.text));
                legend.appendChild(item);
            });
            parts.push(legend);
        }
        const last = open.at === open.steps.length - 1;
        const skipped = last ? skippedText() : null;
        if (skipped) parts.push(node('p', {}, skipped));
        dom.body.replaceChildren(...parts);
        let count = PSL.t('guide.count', {at: open.at + 1, total: open.steps.length});
        if (open.path && open.path.stops.length > 1) {
            count += ' · ' + PSL.t('guide.path.count', {at: open.path.at + 1, total: open.path.stops.length});
        }
        dom.count.textContent = count;
        dom.count.hidden = false;
        dom.quit.hidden = false;
        // Previous stays in the screen: a path's first card on a screen has none.
        dom.prev.hidden = open.at === 0;
        dom.next.hidden = false;
        let action = 'guide.next';
        if (last) action = moreStops() ? 'guide.path.next' : 'guide.close';
        say(dom.next, PSL.t(action));
    }

    /**
     * The guide's own wait or failed text, `guide.<id>.wait.title` for instance, else the shared one. The
     * texts are keyed on the id's last part, `tech` for `recap.tech`, and a source's variant comes first:
     * `guide.incidents.failed.stale.title`.
     * @param {string} part `title` or `body`
     */
    function stateText(part) {
        const own = 'guide.' + open.guide.split('.').pop() + '.' + open.status;
        const generic = 'guide.' + open.status + '.' + part;
        const keys = [open.variant && own + '.' + open.variant + '.' + part, own + '.' + part].filter(Boolean);
        return PSL.t(keys.find(function (key) {
            return PSL.hasText(key);
        }) || generic);
    }

    function paintState() {
        const failed = open.status === 'failed';
        // A screen of a path that shows an error or a refusal hands over to the next screen.
        const refused = failed && moreStops();
        dom.title.textContent = refused ? PSL.t('guide.path.refused.title') : stateText('title');
        const parts = [node('p', {}, refused ? PSL.t('guide.path.refused.body') : stateText('body'))];
        const skipped = failed ? skippedText() : null;
        if (skipped) parts.push(node('p', {}, skipped));
        dom.body.replaceChildren(...parts);
        dom.count.hidden = true;
        dom.prev.hidden = true;
        // A failed guide has one way out, the button that closes it or goes on to the next screen.
        dom.quit.hidden = failed;
        dom.next.hidden = !failed;
        say(dom.next, PSL.t(refused ? 'guide.path.next' : 'guide.close'));
    }

    /** Writes the card, unless a render left it saying the same: the live body would read it out again. */
    function paint() {
        const step = open.status === 'ready' ? open.steps[open.at] : null;
        const painted = JSON.stringify([PSL.lang(), open.status, open.variant, open.at, open.steps.length, step,
            step && (step.marks || []).map(function (mark) {
                return Boolean(anchor(mark.anchor));
            })]);
        if (painted === open.painted) return;
        open.painted = painted;
        dom.quit.textContent = PSL.t('guide.quit');
        say(dom.prev, PSL.t('guide.prev'));
        if (step) paintStep(step);
        else paintState();
    }

    /** The box around the step's elements, in the window's coordinates, or null for a centred card. */
    function box() {
        const step = open.status === 'ready' ? open.steps[open.at] : null;
        const rects = step ? step.anchors.map(anchor).filter(Boolean).map(function (found) {
            return found.getBoundingClientRect();
        }) : [];
        if (!rects.length) return null;
        return rects.reduce(function (sum, rect) {
            return {
                left: Math.min(sum.left, rect.left), top: Math.min(sum.top, rect.top),
                right: Math.max(sum.right, rect.right), bottom: Math.max(sum.bottom, rect.bottom)
            };
        }, {left: Infinity, top: Infinity, right: -Infinity, bottom: -Infinity});
    }

    /**
     * @param {HTMLElement} target
     * @param {number} left
     * @param {number} top
     */
    function at(target, left, top) {
        target.style.left = Math.round(left) + 'px';
        target.style.top = Math.round(top) + 'px';
    }

    /** Puts the cutout, the marks and the card where the step's elements are now. */
    function place() {
        open.frame = 0;
        const width = document.documentElement.clientWidth;
        const height = global.innerHeight;
        const card = dom.card;
        const rect = box();
        dom.marks.replaceChildren();
        if (!rect) {
            at(dom.hole, width / 2, height / 2);
            dom.hole.style.width = '0px';
            dom.hole.style.height = '0px';
            at(card, (width - card.offsetWidth) / 2, Math.max(EDGE, (height - card.offsetHeight) / 2));
            return;
        }
        at(dom.hole, rect.left - PAD, rect.top - PAD);
        dom.hole.style.width = rect.right - rect.left + 2 * PAD + 'px';
        dom.hole.style.height = rect.bottom - rect.top + 2 * PAD + 'px';
        const tall = card.offsetHeight;
        let top = height - EDGE - tall;
        if (rect.bottom + PAD + GAP + tall <= height - EDGE) top = rect.bottom + PAD + GAP;
        else if (rect.top - PAD - GAP - tall >= EDGE) top = rect.top - PAD - GAP - tall;
        const left = Math.min(Math.max(EDGE, rect.left), width - card.offsetWidth - EDGE);
        // Both ends: scrolled up past the element, the card above it would sit under the window's foot.
        at(card, left, Math.min(Math.max(EDGE, top), height - EDGE - tall));
        placeMarks(rect);
    }

    /**
     * One numbered mark left of each marked element. On a narrow screen there is no room left of the
     * line, so the mark stays inside the cutout, and an element that wraps beside others on its line
     * takes its mark after it, where the mark covers nothing.
     * @param {{left: number}} rect
     */
    function placeMarks(rect) {
        const narrow = NARROW.matches;
        let number = 0;
        (open.steps[open.at].marks || []).forEach(function (mark) {
            const found = anchor(mark.anchor);
            if (!found) return;
            number += 1;
            const where = found.getBoundingClientRect();
            const dot = node('span', {class: 'guide-num guide-mark'}, String(number));
            let left = where.left - 24;
            if (narrow && where.left - rect.left > 64) left = where.right + 5;
            else if (narrow) left = Math.max(left, rect.left - PAD + 1);
            at(dot, left, where.top - 1);
            dom.marks.appendChild(dot);
        });
    }

    function schedule() {
        if (open && !open.frame) open.frame = requestAnimationFrame(place);
    }

    /** How far down the window a sticky top bar reaches: it paints over the page, so over the cutout too. */
    function covered() {
        const bar = document.querySelector('.shell-topbar');
        return bar && getComputedStyle(bar).position === 'sticky' ? bar.getBoundingClientRect().bottom : 0;
    }

    /** Brings the step's elements into view, under the top bar and leaving room for the card under them. */
    function reveal() {
        const rect = box();
        if (!rect) return;
        const room = global.innerHeight - (rect.bottom - rect.top) - dom.card.offsetHeight - GAP;
        global.scrollTo({
            top: global.scrollY + rect.top - Math.max(90, covered() + 30, room / 2),
            behavior: reducedMotion() ? 'auto' : 'smooth'
        });
    }

    /**
     * The focus goes back into the card only when it left it, so a redraw never takes it from a button.
     * Out of the wait card it goes to Next, so Enter does not close the guide the wait left on Quit.
     * @param {boolean} woke
     */
    function holdFocus(woke) {
        const active = document.activeElement;
        if (woke) {
            dom.next.focus({preventScroll: true});
            return;
        }
        if (active && dom.card.contains(active) && !(/** @type {HTMLElement} */ (active)).hidden) return;
        (dom.next.hidden ? dom.quit : dom.next).focus({preventScroll: true});
    }

    /**
     * @param {boolean} moved
     * @param {boolean} [woke] the status just turned ready
     */
    function show(moved, woke) {
        paint();
        place();
        if (moved) reveal();
        holdFocus(Boolean(woke));
    }

    /** @param {number} by */
    function move(by) {
        if (open.status !== 'ready') return;
        const to = open.at + by;
        if (to < 0 || to >= open.steps.length) return;
        open.at = to;
        open.id = open.steps[to].id;
        open.from = open.all.indexOf(open.steps[to]);
        show(true);
    }

    /** @param {KeyboardEvent} event */
    function onKey(event) {
        if (event.key === 'Escape') stop();
        else if (event.key === 'ArrowLeft') move(-1);
        else if (event.key === 'ArrowRight') move(1);
        else if (event.key === 'Tab') loopTab(event);
        else return;
        event.preventDefault();
        // The page's own Escape and arrows must not act behind the guide.
        event.stopPropagation();
    }

    /** @param {KeyboardEvent} event */
    function loopTab(event) {
        const stops = [dom.quit, dom.prev, dom.next].filter(function (button) {
            return !button.hidden;
        });
        const index = stops.indexOf(/** @type {HTMLButtonElement} */ (document.activeElement));
        const by = event.shiftKey ? -1 : 1;
        stops[(index + by + stops.length) % stops.length].focus();
    }

    /**
     * Opens the guide over the page, before it reads anything.
     * @param {() => import('../types').GuideSource} source
     * @param {string} guide
     */
    function begin(source, guide) {
        if (open) stop();
        if (!dom) dom = build();
        open = {source, guide, status: 'wait', variant: '', id: null, at: 0, steps: [], all: [], frame: 0, from: 0, painted: '',
            path: null, heading: null};
        SHELL.forEach(function (selector) {
            const part = document.querySelector(selector);
            if (part) part.setAttribute('inert', '');
        });
        dom.root.hidden = false;
        document.addEventListener('keydown', onKey, true);
        // Captured, so a scroll inside a nested scroller moves the cutout too.
        global.addEventListener('scroll', schedule, {capture: true, passive: true});
        global.addEventListener('resize', schedule);
    }

    /**
     * @param {() => import('../types').GuideSource} source
     * @param {string} [guide] the guide's PSL.guideIdFor id, which picks its wait and failed texts
     */
    function start(source, guide = '') {
        begin(source, guide);
        read();
        show(open.status === 'ready');
    }

    /**
     * Walks a path's screen: read at once when the route is already there, else waited on until the hash
     * change has drawn it. The recap's stop carries its view in the route, as the view switch writes it.
     * @param {number} index
     */
    function enter(index) {
        const to = open.path.stops[index];
        Object.assign(open, {source: to.source, guide: to.guide, status: 'wait', variant: '', id: null, at: 0, steps: [],
            all: [], from: 0, painted: '', heading: null});
        open.path.at = index;
        if (global.location.hash === to.hash) {
            read();
            show(open.status === 'ready');
            return;
        }
        open.heading = to.hash;
        show(false);
        // Registered after onRoute's own listener, so the screen is drawn when the stop is read.
        global.addEventListener('hashchange', arrived, {once: true});
        global.location.hash = to.hash;
    }

    /** The route a path headed to has been drawn: the stop is read on it. */
    function arrived() {
        if (!open || open.heading === null) return;
        open.heading = null;
        refresh();
    }

    /** @param {import('../types').GuidePath} path */
    function startPath(path) {
        if (!path.stops.length) return;
        try {
            global.localStorage.setItem(ROLE_KEY, path.role);
        } catch (error) {
            // A browser that keeps nothing still walks the path, and ticks no role next time.
        }
        begin(path.stops[0].source, path.stops[0].guide);
        open.path = {stops: path.stops, skipped: path.skipped, at: 0};
        enter(0);
    }

    function pathStop() {
        return open && open.path ? open.path.stops[open.path.at] : null;
    }

    function role() {
        try {
            return global.localStorage.getItem(ROLE_KEY);
        } catch (error) {
            return null;
        }
    }

    function refresh() {
        if (!open) return;
        const waited = open.status !== 'ready';
        const moved = read();
        show(moved, waited && open.status === 'ready');
    }

    function stop() {
        if (!open) return;
        if (open.frame) cancelAnimationFrame(open.frame);
        open = null;
        document.removeEventListener('keydown', onKey, true);
        global.removeEventListener('scroll', schedule, {capture: true});
        global.removeEventListener('resize', schedule);
        global.removeEventListener('hashchange', arrived);
        SHELL.forEach(function (selector) {
            const part = document.querySelector(selector);
            if (part) part.removeAttribute('inert');
        });
        dom.root.hidden = true;
        dom.marks.replaceChildren();
        const entry = document.getElementById('guide-open');
        if (entry && !entry.hidden) entry.focus();
    }

    function isOpen() {
        return open !== null;
    }

    global.PSL_GUIDE = {start, startPath, pathStop, role, refresh, stop, isOpen};
})(typeof window !== 'undefined' ? window : globalThis);
