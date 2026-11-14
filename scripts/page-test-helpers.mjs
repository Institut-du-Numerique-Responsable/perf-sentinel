// Shared by the page test suites, never served: Playwright, resolved from the site
// or else from the engine's browser suite, and a page opened in a fresh context.
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
let playwright;
try { playwright = require('playwright'); }
catch { playwright = require('/Users/robintrassard/RustroverProjects/perf-sentinel/crates/sentinel-cli/tests/browser/node_modules/playwright'); }
export const { chromium } = playwright;

// The English pages redirect on navigator.language: every context gets this one.
export const LOCALE = 'en-GB';

/**
 * url in a fresh context, every third-party request aborted and recorded, dialogs
 * dismissed and recorded. theme: stored as ps-theme. html: served at url instead of
 * a file. init: scripts run before the page's own, a function or [function, arg].
 */
export async function open(browser, url, { javaScriptEnabled = true, viewport = { width: 1280, height: 900 }, theme, html, init = [] } = {}) {
  const context = await browser.newContext({ javaScriptEnabled, viewport, locale: LOCALE });
  if (theme) await context.addInitScript((t) => { try { localStorage.setItem('ps-theme', t); } catch { /* the page then follows the system */ } }, theme);
  for (const script of init) await (Array.isArray(script) ? context.addInitScript(...script) : context.addInitScript(script));
  const external = [];
  await context.route('**/*', (route) => {
    const u = new URL(route.request().url());
    if (u.hostname === '127.0.0.1') return route.fallback();
    external.push(u.href);
    return route.abort();
  });
  if (html) await context.route(url, (route) => route.fulfill({ body: html, contentType: 'text/html' }));
  context.setDefaultTimeout(10000);
  const page = await context.newPage();
  const dialogs = [];
  page.on('dialog', (d) => { dialogs.push(d.message()); d.dismiss(); });
  await page.goto(url);
  return { page, external, dialogs, close: () => context.close() };
}
