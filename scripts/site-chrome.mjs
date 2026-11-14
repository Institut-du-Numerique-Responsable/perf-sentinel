// Page chrome shared by the build scripts: the vitrine <style> lifted verbatim
// from index.html (theme vars, fonts, footer, hovers), the header with its
// language toggle and theme button, the footer, and the page shell around them.
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const SITE = join(dirname(fileURLToPath(import.meta.url)), '..');
const STYLE = readFileSync(join(SITE, 'index.html'), 'utf8').match(/<style>[\s\S]*?<\/style>/)[0];

const ICON_SYSTEM = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="13" rx="2"></rect><path d="M8 21h8M12 17v4"></path></svg>';
const ICON_SUN = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="4"></circle><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"></path></svg>';
const ICON_MOON = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"></path></svg>';
const GH = '<svg viewBox="0 0 24 24" width="17" height="17" fill="currentColor" aria-hidden="true"><path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12"></path></svg>';

const logo = '<img data-logo="light" src="/assets/logo-h-light.svg" alt="perf sentinel" style="height:38px;width:auto"><img data-logo="dark" src="/assets/logo-h-dark.svg" alt="perf sentinel" style="height:38px;width:auto"><img data-logo="light" src="/assets/logo-h-light.svg" alt="perf sentinel" style="height:40px;width:auto;display:none">';

const FT = {
  fr: { tagline: 'Détecteur d’anti-patterns d’I/O au niveau protocole, auto-hébergeable et carbon-aware.', product: 'Produit', docs: 'Documentation', project: 'Projet', detection: 'Détection', execution: 'Déploiement', perf: 'Performance', greenops: 'GreenOps', comparison: 'Comparatif', quickstart: 'Démarrage rapide', cli: 'Référence CLI', config: 'Configuration', method: 'Méthodologie GreenOps', license: 'Licence AGPL-3.0', security: 'Sécurité', copyright: '© 2026 Robin Trassard et contributeurs · Institut du Numérique Responsable · AGPL-3.0 · Page éco-conçue, sans script de tracking', legal: 'Mentions légales', privacy: 'Confidentialité', credit: 'Logo & bannière, ' },
  en: { tagline: 'Protocol-level I/O anti-pattern detector, self-hostable and carbon-aware.', product: 'Product', docs: 'Documentation', project: 'Project', detection: 'Detection', execution: 'Deploy', perf: 'Performance', greenops: 'GreenOps', comparison: 'Comparison', quickstart: 'Quickstart', cli: 'CLI reference', config: 'Configuration', method: 'GreenOps methodology', license: 'AGPL-3.0 license', security: 'Security', copyright: '© 2026 Robin Trassard and contributors · Institut du Numérique Responsable · AGPL-3.0 · Eco-designed page, no tracking script', legal: 'Legal notice', privacy: 'Privacy', credit: 'Logo & banner, ' },
};
const LEGAL_HREF = { fr: '/mentions-legales', en: '/legal-notice' };
const PRIV_HREF = { fr: '/confidentialite', en: '/privacy-policy' };
export const HOME_HREF = { fr: '/fr/', en: '/' };
export const GUIDE_HREF = { fr: '/fr/guide', en: '/guide' };
const HOME = { fr: 'Accueil', en: 'Home' };
const LANGSW = { fr: 'EN', en: 'FR' };
const OTHER = { fr: 'en', en: 'fr' };

const GLOBE = '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="9"></circle><path d="M3 12h18"></path><path d="M12 3c2.5 2.4 4 5.6 4 9s-1.5 6.6-4 9c-2.5-2.4-4-5.6-4-9s1.5-6.6 4-9z"></path></svg>';

// Language control: globe + horizontal-slide reveal (active label at rest,
// target on hover). An <a> so the static legal pages navigate to the other
// language; the tail script also persists the choice for the SPA pages.
const langCtl = (href, lang) => {
  const active = lang === 'fr' ? 'FR' : 'EN', target = LANGSW[lang];
  const aria = lang === 'fr' ? 'Changer de langue' : 'Switch language';
  return `<a data-plain data-lang-switch href="${href}" class="ps-lang-btn" aria-label="${aria}" title="${aria}"><span class="ps-lang-ico">${GLOBE}</span><span class="ps-lang-win"><span class="ps-lang-track"><span class="ps-lang-cell ps-lang-cur">${active}</span><span class="ps-lang-cell ps-lang-next">${target}</span></span></span></a>`;
};

export function header(lang, otherHref) {
  return `<header style="position:sticky;top:0;z-index:50;background:color-mix(in srgb,var(--bg) 85%,transparent);backdrop-filter:blur(10px);border-bottom:1px solid var(--border)"><div style="max-width:1120px;margin:0 auto;padding:14px 28px;display:flex;align-items:center;gap:18px">` +
    `<a href="${HOME_HREF[lang]}" style="display:flex;align-items:center;flex:none">${logo}</a>` +
    `<a class="ps-home-lnk" href="${HOME_HREF[lang]}" style="font-size:14px;color:var(--text-2);text-decoration:none">&larr; ${HOME[lang]}</a>` +
    `<div style="margin-left:auto;display:flex;align-items:center;gap:10px">` +
    langCtl(otherHref, lang) +
    `<button id="themeBtn" class="ps-th-btn" aria-label="Theme"><span class="ps-th-ico">${ICON_SYSTEM}</span><span class="ps-th-lbl">${lang === 'fr' ? 'Système' : 'System'}</span></button>` +
    `<a data-plain href="https://github.com/Institut-du-Numerique-Responsable/perf-sentinel" aria-label="Perf Sentinel on GitHub" style="display:flex;align-items:center;gap:8px;height:34px;box-sizing:border-box;font-size:13px;font-weight:600;color:#FFFFFF;background:#24292F;border:1px solid rgba(240,246,252,.18);border-radius:8px;padding:0 12px">${GH}<span class="gh-txt">GitHub</span></a>` +
    `</div></div></header>`;
}

const col = (title, links) =>
  `<div><div style="font-size:13px;font-weight:600;color:var(--text);margin-bottom:13px">${title}</div><div style="display:flex;flex-direction:column;gap:9px;font-size:14px;color:var(--text-2)">${links.map(([h, t]) => `<a href="${h}">${t}</a>`).join('')}</div></div>`;

export function footer(lang) {
  const f = FT[lang];
  return `<footer style="border-top:1px solid var(--border);background:var(--surface-2)"><div style="max-width:1120px;margin:0 auto;padding:48px 28px 36px">` +
    `<div data-grid="ftcols" style="display:grid;grid-template-columns:1.5fr 1fr 1fr 1fr;gap:32px">` +
    `<div><img data-logo="light" src="/assets/logo-h-light.svg" alt="perf sentinel" style="height:40px;width:auto"><img data-logo="dark" src="/assets/logo-h-dark.svg" alt="perf sentinel" style="height:40px;width:auto"><p style="margin:16px 0 0;font-size:13.5px;color:var(--text-2);max-width:240px;line-height:1.55">${f.tagline}</p></div>` +
    col(f.product, [[HOME_HREF[lang] + '#detection', f.detection], [HOME_HREF[lang] + '#modes', f.execution], [HOME_HREF[lang] + '#perf', f.perf], [HOME_HREF[lang] + '#greenops', f.greenops], [HOME_HREF[lang] + '#comparatif', f.comparison]]) +
    col(f.docs, [[GUIDE_HREF[lang] + '#quickstart', f.quickstart], [GUIDE_HREF[lang] + '#cli', f.cli], [GUIDE_HREF[lang] + '#config', f.config], [GUIDE_HREF[lang] + '#metrics', f.method]]) +
    col(f.project, [['https://github.com/Institut-du-Numerique-Responsable/perf-sentinel', 'GitHub'], ['https://crates.io/crates/perf-sentinel', 'crates.io'], ['https://docs.rs/perf-sentinel-core', 'docs.rs'], [HOME_HREF[lang] + '#license', f.license], [HOME_HREF[lang] + '#security', f.security]]) +
    `</div>` +
    `<div style="display:flex;justify-content:space-between;gap:18px;margin-top:26px;flex-wrap:wrap;font-size:12.5px;color:var(--text-2)">` +
    `<div style="display:flex;flex-direction:column;gap:6px"><span>${f.copyright}</span><span style="display:flex;gap:14px"><a href="${LEGAL_HREF[lang]}">${f.legal}</a><a href="${PRIV_HREF[lang]}">${f.privacy}</a></span></div>` +
    `<span>${f.credit}<a href="https://www.linkedin.com/in/gwendoline-meignen-b0224873/" target="_blank" rel="noopener" id="ftCredit">Gwendoline Meignen</a></span>` +
    `</div></div></footer>`;
}

const legalCss = `<style>@media(max-width:560px){.gh-txt,.ps-home-lnk{display:none}header>div{gap:10px!important;padding-left:8px!important;padding-right:8px!important}header>div>div{gap:8px!important}}@media(max-width:380px){header img[data-logo]{height:30px!important}}.legal-main{max-width:760px;margin:0 auto;padding:56px 28px 84px}.legal-main h1{font-size:30px;line-height:1.2;margin:0 0 6px;color:var(--text)}.legal-main h2{font-size:18px;margin:36px 0 10px;color:var(--text)}.legal-main p,.legal-main li{font-size:15px;line-height:1.66;color:var(--text-2)}.legal-main .upd{font-size:13px;margin:0 0 30px}.legal-main a{color:var(--accent-strong)}[data-ps-root]:not([data-theme="dark"]) [style*="color:var(--accent)"],[data-ps-root]:not([data-theme="dark"]) [style*="color: var(--accent)"],[data-ps-root]:not([data-theme="dark"]) [style*="color:#0ba671"],[data-ps-root]:not([data-theme="dark"]) [style*="color: #0ba671"]{color:var(--accent-strong)!important}</style>`;

// Anti-flash: only concrete dark/light may land on data-theme; a stored
// 'system' (or nothing) resolves against the OS preference first.
const initScript = `<script>(function(){var t=null;try{t=localStorage.getItem('ps-theme')}catch(e){}if(t!=='dark'&&t!=='light'){try{t=window.matchMedia&&window.matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light'}catch(e){t='light'}}document.currentScript.parentElement.setAttribute('data-theme',t);})();</script>`;
const tailScript = (lang) => {
  const names = lang === 'fr' ? "{system:'Système',light:'Clair',dark:'Sombre'}" : "{system:'System',light:'Light',dark:'Dark'}";
  const aria = lang === 'fr' ? "'Changer de thème (actuel : '" : "'Switch theme (current: '";
  return `<script>(function(){var r=document.querySelector('[data-ps-root]'),b=document.getElementById('themeBtn');var TN=${names};var TI={system:'${ICON_SYSTEM}',light:'${ICON_SUN}',dark:'${ICON_MOON}'};var mq=null;try{mq=window.matchMedia('(prefers-color-scheme: dark)')}catch(e){}function getMode(){var v=null;try{v=localStorage.getItem('ps-theme')}catch(e){}return v==='dark'||v==='light'||v==='system'?v:'system'}function resT(m){return m==='system'?(mq&&mq.matches?'dark':'light'):m}function applyMode(m){r.setAttribute('data-theme',resT(m));if(b){b.innerHTML='<span class="ps-th-ico">'+TI[m]+'</span><span class="ps-th-lbl">'+TN[m]+'</span>';var a=${aria}+TN[m]+')';b.title=a;b.setAttribute('aria-label',a)}}applyMode(getMode());if(b)b.addEventListener('click',function(){var o=['system','light','dark'],n=o[(o.indexOf(getMode())+1)%3];try{localStorage.setItem('ps-theme',n)}catch(e){}applyMode(n)});if(mq){try{mq.addEventListener('change',function(){if(getMode()==='system')r.setAttribute('data-theme',resT('system'))})}catch(e){}}var lb=document.querySelector('[data-lang-switch]');if(lb)lb.addEventListener('click',function(){try{localStorage.setItem('ps-lang','${OTHER[lang]}');}catch(e){}});})();</script>`;
};

export function page(lang, otherHref, title, desc, main, { robots = 'index,follow', head = '' } = {}) {
  return `<!doctype html><html lang="${lang}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">` +
    `<title>${title} · Perf Sentinel</title><meta name="description" content="${desc}"><meta name="robots" content="${robots}">` +
    `<meta property="og:type" content="website"><meta property="og:title" content="${title} · Perf Sentinel"><meta property="og:description" content="${desc}"><meta property="og:image" content="https://perf-sentinel.dev/assets/og-banner.png"><meta property="og:image:width" content="1200"><meta property="og:image:height" content="427"><meta property="og:image:alt" content="Perf Sentinel"><meta name="theme-color" content="#0BA671"><meta name="twitter:card" content="summary_large_image"><meta name="twitter:image" content="https://perf-sentinel.dev/assets/og-banner.png">` +
    `<link rel="icon" type="image/svg+xml" href="/assets/favicon.svg"><link rel="stylesheet" href="/fonts/fonts.css">${STYLE}${legalCss}${head}</head>` +
    `<body><div data-ps-root data-theme="light">${initScript}<div style="min-height:100vh;display:flex;flex-direction:column;background-color:var(--bg);color:var(--text)">` +
    `${header(lang, otherHref)}<main class="legal-main" style="flex:1 0 auto">${main}</main>${footer(lang)}</div></div>${tailScript(lang)}</body></html>`;
}
