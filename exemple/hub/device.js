// The terminal sign-in approval page, served by GET /device only. The code is always typed:
// nothing here reads location.search or location.hash, so no link can fill it in. The texts
// come from the launcher's tables, in the language the server wrote into <html lang>.
(function () {
    'use strict';

    // The launcher's theme, resolved before the first paint: tokens.css has no palette
    // without the attribute. This page allows no inline script, so it lives here.
    let stored = null;
    try {
        stored = sessionStorage.getItem('perf-sentinel:theme') || localStorage.getItem('perf-sentinel:theme');
    } catch (e) {
        stored = null;
    }
    if (stored !== 'light' && stored !== 'dark') {
        stored = matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
    }
    document.documentElement.dataset.theme = stored;
    // The page's language, not a choice: storing it would override the launcher's own.
    PSL.setLang(document.documentElement.lang, false);

    async function send(path, body) {
        const response = await fetch(path, {
            method: 'POST',
            headers: {'content-type': 'application/json'},
            body: JSON.stringify(body)
        });
        // A lapsed session signs in again, and so does one too close to its end (409).
        if (response.status === 401 || response.status === 409) location.reload();
        const payload = await response.json().catch(function () {
            return null;
        });
        return {status: response.status, payload: payload};
    }

    document.addEventListener('DOMContentLoaded', function () {
        const form = document.getElementById('device-form');
        const input = document.getElementById('device-code');
        const details = document.getElementById('device-details');
        const networkAlert = document.getElementById('device-alert');
        const approve = document.getElementById('device-approve');
        const deny = document.getElementById('device-deny');
        const result = document.getElementById('device-result');
        let verified = null;
        let armed = false;

        // The live region stays in the page and only its content changes: a region shown and
        // filled in the same moment is not announced by every screen reader.
        function say(text, tone) {
            const banner = document.createElement('div');
            banner.className = 'banner';
            banner.dataset.tone = tone;
            banner.textContent = text;
            result.replaceChildren(banner);
        }

        function refused(answer) {
            say(PSL.problemText(answer.payload, answer.status, PSL.t('device.answered', {status: answer.status})), 'crit');
        }

        // No answer (network down, Hub restarting) or one the page could not use: never silent.
        function failed(error) {
            say(PSL.t('device.failed', {error: String(error?.message || error)}), 'crit');
        }

        fetch('/api/access', {headers: {accept: 'application/json'}}).then(function (response) {
            return response.ok ? response.json() : null;
        }).then(function (access) {
            if (access) document.getElementById('device-intro').textContent = PSL.deviceIntro(access.expires_at_ms);
        }).catch(function () {
            // The intro keeps its generic end.
        });

        form.addEventListener('submit', function (event) {
            event.preventDefault();
            result.replaceChildren();
            details.hidden = true;
            send('/api/device/verify', {user_code: input.value}).then(function (answer) {
                if (answer.status !== 200) return refused(answer);
                verified = answer.payload;
                armed = false;
                approve.textContent = PSL.t('device.approve');
                document.getElementById('device-request').textContent = PSL.deviceRequestLine(verified, Date.now());
                const warning = PSL.deviceNetworkAlert(verified);
                networkAlert.textContent = warning || '';
                networkAlert.hidden = !warning;
                delete approve.dataset.armed;
                details.hidden = false;
                // Where and when the code came from is what to read before deciding.
                document.getElementById('device-request').focus();
            }).catch(failed);
        });

        function decide(approved) {
            send('/api/device/approve', {user_code: verified.user_code, approve: approved}).then(function (answer) {
                if (answer.status !== 204) return refused(answer);
                form.hidden = true;
                details.hidden = true;
                say(PSL.t(approved ? 'device.approved' : 'device.denied'), approved ? 'ok' : 'muted');
                // The buttons that had the focus are gone: the outcome takes it.
                result.focus();
            }).catch(failed);
        }

        approve.addEventListener('click', function () {
            if (PSL.deviceApproveStep(verified.same_network, armed) === 'arm') {
                armed = true;
                approve.textContent = PSL.t('device.approve_anyway');
                approve.dataset.armed = 'true';
                say(PSL.t('device.armed'), 'warn');
                return;
            }
            decide(true);
        });
        deny.addEventListener('click', function () {
            decide(false);
        });
    });
})();
