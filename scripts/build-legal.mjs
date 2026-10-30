// Generates the standalone legal pages, the 404 and the share card verifier, FR + EN:
//   FR: mentions-legales.html, confidentialite.html, fr/404.html, fr/verifier.html
//   EN: legal-notice.html, privacy-policy.html, 404.html, verifier.html
// Lifts the vitrine <style> verbatim so the chrome (theme vars, fonts, footer,
// hovers) stays identical without duplicating CSS. Each page has a language
// toggle linking to its counterpart and persisting ps-lang.
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { COPY } from '../assets/verifier/copy.js';

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
const HOME_HREF = { fr: '/fr/', en: '/' };
const GUIDE_HREF = { fr: '/fr/guide', en: '/guide' };
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

function header(lang, otherHref) {
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

function footer(lang) {
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

function page(lang, otherHref, title, desc, main, { robots = 'index,follow', head = '' } = {}) {
  return `<!doctype html><html lang="${lang}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">` +
    `<title>${title} · Perf Sentinel</title><meta name="description" content="${desc}"><meta name="robots" content="${robots}">` +
    `<meta property="og:type" content="website"><meta property="og:title" content="${title} · Perf Sentinel"><meta property="og:description" content="${desc}"><meta property="og:image" content="https://perf-sentinel.dev/assets/og-banner.png"><meta property="og:image:width" content="1200"><meta property="og:image:height" content="427"><meta property="og:image:alt" content="Perf Sentinel"><meta name="theme-color" content="#0BA671"><meta name="twitter:card" content="summary_large_image"><meta name="twitter:image" content="https://perf-sentinel.dev/assets/og-banner.png">` +
    `<link rel="icon" type="image/svg+xml" href="/assets/favicon.svg"><link rel="stylesheet" href="/fonts/fonts.css">${STYLE}${legalCss}${head}</head>` +
    `<body><div data-ps-root data-theme="light">${initScript}<div style="min-height:100vh;display:flex;flex-direction:column;background-color:var(--bg);color:var(--text)">` +
    `${header(lang, otherHref)}<main class="legal-main" style="flex:1 0 auto">${main}</main>${footer(lang)}</div></div>${tailScript(lang)}</body></html>`;
}

const EMAIL = '<a href="mailto:robin.trassard@gmail.com">robin.trassard@gmail.com</a>';

// --- content ---
const mentionsFR =
  `<h1>Mentions légales</h1><p class="upd">Dernière mise à jour : juin 2026</p>` +
  `<h2>Éditeur du site</h2><p>Le présent site est édité par Robin Trassard, à titre personnel (particulier non-professionnel).<br>Contact : ${EMAIL}.</p>` +
  `<h2>Directeur de la publication</h2><p>Robin Trassard.</p>` +
  `<h2>Hébergeur</h2><p>Le site est hébergé par GitHub Pages, service de GitHub, Inc., 88 Colin P. Kelly Jr. Street, San Francisco, CA 94107, États-Unis (<a href="https://github.com" target="_blank" rel="noopener">github.com</a>).</p>` +
  `<p>Le nom de domaine perf-sentinel.dev est enregistré auprès d'OVH SAS, 2 rue Kellermann, 59100 Roubaix, France (<a href="https://www.ovhcloud.com" target="_blank" rel="noopener">ovhcloud.com</a>).</p>` +
  `<h2>Propriété intellectuelle</h2><p>Le logiciel Perf Sentinel est distribué sous licence libre GNU AGPL-3.0 (<a href="https://www.gnu.org/licenses/agpl-3.0.html" target="_blank" rel="noopener">texte de la licence</a>) ; son code source est disponible sur <a href="https://github.com/Institut-du-Numerique-Responsable/perf-sentinel" target="_blank" rel="noopener">GitHub</a>. Le logo et la bannière sont l'œuvre de Gwendoline Meignen. Les autres contenus du site (textes, mise en page) sont la propriété de l'éditeur, sauf mention contraire. Les logos des langages, frameworks et ORM présentés dans la section « vos technologies » appartiennent à leurs détenteurs respectifs et indiquent seulement la compatibilité de Perf Sentinel, sans affiliation. Il en va de même pour les logos des outils de mesure de la section « sources de mesure » (Alumet, Scaphandre, Kepler, DMTF Redfish, Electricity Maps), qui indiquent seulement une intégration. Le logo Entity Framework Core provient du <a href="https://github.com/campusMVP/dotnetCoreLogoPack" target="_blank" rel="noopener">dotnetCore Logo Pack</a> de campusMVP, sous licence <a href="https://creativecommons.org/licenses/by-sa/4.0/deed.fr" target="_blank" rel="noopener">CC BY-SA 4.0</a>, converti au format SVG et recoloré aux couleurs de .NET.</p>` +
  `<h2>Responsabilité</h2><p>Ce site a une vocation informationnelle. L'éditeur s'efforce de fournir des informations exactes mais ne saurait garantir leur exhaustivité ni l'absence d'erreurs. L'utilisation du logiciel Perf Sentinel relève de la seule responsabilité de l'utilisateur, dans les conditions de la licence AGPL-3.0.</p>` +
  `<h2>Données personnelles</h2><p>Ce site ne collecte aucune donnée personnelle et n'utilise aucun cookie ni traceur. Pour le détail, voir la <a href="/confidentialite">politique de confidentialité</a>.</p>`;

const mentionsEN =
  `<h1>Legal notice</h1><p class="upd">Last updated: June 2026</p>` +
  `<h2>Publisher</h2><p>This site is published by Robin Trassard, as a private individual (non-professional).<br>Contact: ${EMAIL}.</p>` +
  `<h2>Publication director</h2><p>Robin Trassard.</p>` +
  `<h2>Host</h2><p>This site is hosted by GitHub Pages, a service of GitHub, Inc., 88 Colin P. Kelly Jr. Street, San Francisco, CA 94107, USA (<a href="https://github.com" target="_blank" rel="noopener">github.com</a>).</p>` +
  `<p>The perf-sentinel.dev domain name is registered with OVH SAS, 2 rue Kellermann, 59100 Roubaix, France (<a href="https://www.ovhcloud.com" target="_blank" rel="noopener">ovhcloud.com</a>).</p>` +
  `<h2>Intellectual property</h2><p>The Perf Sentinel software is distributed under the GNU AGPL-3.0 free license (<a href="https://www.gnu.org/licenses/agpl-3.0.html" target="_blank" rel="noopener">license text</a>); its source code is available on <a href="https://github.com/Institut-du-Numerique-Responsable/perf-sentinel" target="_blank" rel="noopener">GitHub</a>. The logo and banner are the work of Gwendoline Meignen. Other site content (text, layout) is the property of the publisher unless otherwise stated. The language, framework and ORM logos shown in the "your stack" section belong to their respective owners and only indicate that Perf Sentinel works with them, with no affiliation. The same applies to the measurement tool logos in the "measurement sources" section (Alumet, Scaphandre, Kepler, DMTF Redfish, Electricity Maps), which only indicate an integration. The Entity Framework Core logo comes from the <a href="https://github.com/campusMVP/dotnetCoreLogoPack" target="_blank" rel="noopener">dotnetCore Logo Pack</a> by campusMVP, licensed under <a href="https://creativecommons.org/licenses/by-sa/4.0/" target="_blank" rel="noopener">CC BY-SA 4.0</a>, converted to SVG and recoloured to the .NET purple.</p>` +
  `<h2>Liability</h2><p>This site is informational. The publisher strives to provide accurate information but cannot guarantee its completeness or the absence of errors. Use of the Perf Sentinel software is the sole responsibility of the user, under the terms of the AGPL-3.0 license.</p>` +
  `<h2>Personal data</h2><p>This site collects no personal data and uses no cookie or tracker. For details, see the <a href="/privacy-policy">privacy policy</a>.</p>`;

const confidFR =
  `<h1>Politique de confidentialité</h1><p class="upd">Dernière mise à jour : octobre 2026</p>` +
  `<p>La protection de votre vie privée est prise au sérieux. Cette page décrit, en clair, les (rares) données que ce site traite, conformément au Règlement général sur la protection des données (RGPD).</p>` +
  `<h2>Aucune collecte de données</h2><p>Ce site est purement informationnel. Il ne comporte ni formulaire, ni compte utilisateur, ni inscription à une newsletter. Aucune donnée personnelle n'est collectée directement par l'éditeur.</p>` +
  `<h2>Cookies et traceurs</h2><p>Ce site ne dépose aucun cookie et n'utilise aucun traceur ni outil de mesure d'audience (pas de Google Analytics, Matomo, Plausible, etc.). Aucun consentement n'est donc requis et aucun bandeau cookies n'est nécessaire.</p>` +
  `<h2>Stockage local (préférences)</h2><p>Vos préférences d'affichage (thème clair/sombre, langue et profil de lecture) sont enregistrées dans le stockage local (localStorage) de votre navigateur. La page d'accueil y garde aussi, pendant six heures, les derniers numéros de version et nombres d'étoiles lus sur GitHub, pour ne pas les redemander à chaque visite. Ces informations restent sur votre appareil, ne sont jamais transmises à un serveur, et vous pouvez les effacer à tout moment en vidant les données de site de votre navigateur.</p>` +
  `<h2>Polices de caractères</h2><p>Les polices sont hébergées directement sur le site. Aucune requête n'est envoyée à un service tiers (pas de Google Fonts), donc aucune donnée n'est transmise de ce fait.</p>` +
  `<h2>Requêtes vers GitHub</h2><p>Pour afficher les dernières versions publiées et le nombre d'étoiles des dépôts, la page d'accueil lit deux petits fichiers sur raw.githubusercontent.com et interroge l'API publique de GitHub (api.github.com). Le guide charge aussi des images depuis raw.githubusercontent.com. Ces requêtes partent sans cookie ni identifiant, mais comme toute requête web, elles transmettent votre adresse IP à GitHub, qui héberge déjà le site.</p>` +
  `<h2>Page de vérification des cartes</h2><p>La <a href="/fr/verifier">page de vérification</a> contrôle une carte à partager Perf Sentinel dans votre navigateur. Les données de la carte se trouvent après le <code>#</code> du lien : elles restent dans le navigateur et n'atteignent jamais le serveur. Un fichier déposé sur la page est lu dans le navigateur et n'est jamais envoyé. Seul le bouton « Vérifier … auprès de Cloudflare » contacte un tiers : il interroge Cloudflare DNS, puis Google Public DNS si Cloudflare ne répond pas, pour lire la clé publiée par l'organisation que la carte nomme. Le résolveur interrogé voit le nom de domaine de cette organisation et votre adresse IP. Voir la <a href="https://developers.cloudflare.com/1.1.1.1/privacy/public-dns-resolver/" target="_blank" rel="noopener">politique de confidentialité de Cloudflare DNS</a> et celle de <a href="https://developers.google.com/speed/public-dns/privacy" target="_blank" rel="noopener">Google Public DNS</a>.</p>` +
  `<h2>Hébergement et journaux</h2><p>Comme tout hébergeur web, GitHub Pages peut enregistrer des données techniques de connexion (adresse IP, type de navigateur) à des fins de fonctionnement et de sécurité. Ce traitement relève de GitHub ; voir la <a href="https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement" target="_blank" rel="noopener">politique de confidentialité de GitHub</a>.</p>` +
  `<h2>Liens externes</h2><p>Le site renvoie vers des services tiers (GitHub, crates.io, docs.rs, LinkedIn, le site de l'Institut du Numérique Responsable) qui disposent de leurs propres politiques de confidentialité. L'éditeur n'est pas responsable de leurs pratiques.</p>` +
  `<h2>Vos droits</h2><p>Conformément au RGPD, vous disposez d'un droit d'accès, de rectification et d'effacement de vos données. Le site ne détenant aucune donnée personnelle vous concernant, ces demandes concernent surtout l'hébergeur. Pour toute question : ${EMAIL}. Vous pouvez aussi introduire une réclamation auprès de la CNIL (<a href="https://www.cnil.fr" target="_blank" rel="noopener">cnil.fr</a>).</p>`;

const confidEN =
  `<h1>Privacy policy</h1><p class="upd">Last updated: October 2026</p>` +
  `<p>Your privacy is taken seriously. This page describes, in plain terms, the (few) data this site processes, in line with the General Data Protection Regulation (GDPR).</p>` +
  `<h2>No data collection</h2><p>This site is purely informational. It has no form, no user account, and no newsletter signup. No personal data is collected directly by the publisher.</p>` +
  `<h2>Cookies and trackers</h2><p>This site sets no cookie and uses no tracker or audience-measurement tool (no Google Analytics, Matomo, Plausible, etc.). No consent is therefore required and no cookie banner is needed.</p>` +
  `<h2>Local storage (preferences)</h2><p>Your display preferences (light/dark theme, language and reading profile) are stored in your browser's local storage. The home page also keeps the latest version numbers and star counts read from GitHub there for six hours, so it does not ask again on every visit. This information stays on your device, is never sent to a server, and you can clear it at any time by clearing the site data in your browser.</p>` +
  `<h2>Fonts</h2><p>Fonts are hosted directly on the site. No request is sent to a third-party service (no Google Fonts), so no data is transmitted as a result.</p>` +
  `<h2>Requests to GitHub</h2><p>To show the latest published versions and the repositories' star counts, the home page reads two small files from raw.githubusercontent.com and queries GitHub's public API (api.github.com). The guide also loads images from raw.githubusercontent.com. These requests carry no cookie or identifier, but like any web request they send your IP address to GitHub, which already hosts the site.</p>` +
  `<h2>Card verification page</h2><p>The <a href="/verifier">verification page</a> checks a Perf Sentinel share card in your browser. The card data sits after the <code>#</code> in the link: it stays in your browser and never reaches the server. A file you drop on the page is read in your browser and never uploaded. Only the "Check … with Cloudflare" button contacts a third party: it asks Cloudflare DNS, then Google Public DNS if Cloudflare does not answer, for the key published by the organisation the card names. The resolver asked sees that organisation's domain name and your IP address. See the <a href="https://developers.cloudflare.com/1.1.1.1/privacy/public-dns-resolver/" target="_blank" rel="noopener">Cloudflare DNS privacy policy</a> and the <a href="https://developers.google.com/speed/public-dns/privacy" target="_blank" rel="noopener">Google Public DNS privacy notice</a>.</p>` +
  `<h2>Hosting and logs</h2><p>Like any web host, GitHub Pages may record technical connection data (IP address, browser type) for operational and security purposes. This processing is GitHub's; see <a href="https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement" target="_blank" rel="noopener">GitHub's privacy statement</a>.</p>` +
  `<h2>External links</h2><p>The site links to third-party services (GitHub, crates.io, docs.rs, LinkedIn, the Institut du Numérique Responsable website) which have their own privacy policies. The publisher is not responsible for their practices.</p>` +
  `<h2>Your rights</h2><p>Under the GDPR, you have the right to access, rectify and erase your data. As the site holds no personal data about you, such requests mainly concern the host. For any question: ${EMAIL}. You may also lodge a complaint with the CNIL, the French data-protection authority (<a href="https://www.cnil.fr" target="_blank" rel="noopener">cnil.fr</a>).</p>`;

// 404. GitHub Pages only serves the root /404.html, so the English one carries
// a redirect to the FR twin for readers whose resolved language is French.
const NF = {
  fr: { title: 'Page introuvable', desc: 'Cette page du site Perf Sentinel n’existe pas ou a été déplacée.', h1: 'Cette page n’existe pas', p: 'Le lien est peut-être cassé, ou la page a été déplacée depuis.', home: 'Accueil', docs: 'Documentation' },
  en: { title: 'Page not found', desc: 'This Perf Sentinel page does not exist or has moved.', h1: 'This page does not exist', p: 'The link may be broken, or the page has moved since.', home: 'Home', docs: 'Documentation' },
};
const notFound = (lang) => {
  const n = NF[lang];
  return `<div style="text-align:center">` +
    `<div style="font-family:'JetBrains Mono',monospace;font-size:clamp(52px,10vw,108px);line-height:1;font-weight:600;letter-spacing:-.03em;word-spacing:-.35em;color:var(--accent)">// 404</div>` +
    `<h1 style="margin:22px 0 0;font-size:clamp(24px,3vw,32px);line-height:1.2;letter-spacing:-.018em">${n.h1}</h1>` +
    `<p style="margin:14px auto 0;max-width:460px">${n.p}</p>` +
    `<div style="display:flex;gap:14px;justify-content:center;flex-wrap:wrap;margin-top:32px">` +
    `<a data-plain href="${HOME_HREF[lang]}" style="font-size:15px;font-weight:600;color:var(--on-accent);background:#0b523a;border-radius:9px;padding:13px 22px">${n.home}</a>` +
    `<a data-plain href="${GUIDE_HREF[lang]}" style="font-size:15px;font-weight:600;color:var(--text);background:var(--surface);border:1px solid var(--border);border-radius:9px;padding:13px 22px">${n.docs}</a>` +
    `</div></div>`;
};
// Centers the block in whatever height the viewport leaves between header and footer.
const nfCss = `<style>.legal-main{display:flex;align-items:center;justify-content:center;padding:48px 28px}</style>`;
// A missing /fr/ or /docs/fr/ URL keeps its language, other missing URLs follow the stored choice.
const frRedirect = `<script>(function(){var p=location.pathname,l=p.indexOf('/fr/')===0||p.indexOf('/docs/fr/')===0?'fr':null;if(!l)try{l=localStorage.getItem('ps-lang')}catch(e){}if(l!=='fr'&&l!=='en')l=(navigator.language||'').toLowerCase().indexOf('fr')===0?'fr':'en';if(l==='fr')location.replace('/fr/404');})();</script>`;
// /verifier follows the stored choice, else the browser language; /fr/verifier never redirects.
const verifierRedirect = `<script>(function(){var l=null;try{l=localStorage.getItem('ps-lang')}catch(e){}if(l!=='fr'&&l!=='en')l=(navigator.language||'').toLowerCase().indexOf('fr')===0?'fr':'en';if(l==='fr')location.replace('/fr/verifier'+location.hash);})();</script>`;

// Share card verifier: static frame here, every state drawn by assets/verifier/page.js.
const vIco = (n) => `<span data-ico="${n}" aria-hidden="true"></span>`;
function verifierMain(lang) {
  const t = COPY[lang];
  const [lead, action] = t.drop.split(/(?<=, (?:or|ou) )/);
  return `<h1>${t.title}</h1><p class="vf-intro">${t.intro}</p>` +
    `<div class="vf-res" id="verifier-result" data-state="empty" aria-live="polite"></div>` +
    `<label class="vf-drop" id="verifier-drop"><input type="file" id="verifier-file" accept=".png,.svg,.jxl,.pdf,.jpg,.jpeg,.webp,image/png,image/svg+xml,image/jxl,application/pdf,image/jpeg,image/webp">${vIco('upload')}<span><strong>${lead}<u>${action}</u></strong><small>${t.dropSub}</small></span></label>` +
    `<p class="vf-phone" id="verifier-phone">${t.phone}</p>` +
    `<details class="vf-lim" id="verifier-limits"><summary>${vIco('chevron-right')}${t.limitsLink}</summary><p>${t.limitLevel1}</p><p>${t.limitStolenKey}</p><p>${t.limitLinkedIn}</p><p>${t.limitRepost}</p><a class="vf-meth" href="${t.methodHref}">${t.method}</a></details>` +
    `<script type="module" src="/assets/verifier/page.js"></script>`;
}
// The approved mockup (card-site Task 1), inside the .legal-main frame. Level 1 never uses the accent.
const verifierCss = `<style>${['shield-check', 'shield-alert', 'link-2-off', 'circle-alert', 'image-off', 'file-x', 'upload', 'chevron-right'].map((n) => `[data-ico="${n}"]{--ico:url(/assets/icons/${n}.svg)}`).join('')}` +
  `.legal-main .vf-intro{margin:14px 0 0;max-width:66ch;font-size:15.5px;line-height:1.65}` +
  `.legal-main .vf-phone{margin:12px 0 0;max-width:66ch;font-size:14.5px;line-height:1.6;color:var(--text-2)}` +
  `.vf-res{margin-top:30px;background:var(--surface);border:1px solid var(--border);border-radius:14px;overflow:hidden}` +
  `.vf-res[data-state="empty"]{display:none}` +
  `.vf-hd{display:flex;align-items:flex-start;justify-content:space-between;gap:18px;padding:22px 24px 20px;color:var(--text-2)}` +
  `.vf-hd [data-ico]{flex:none;width:26px;height:26px;margin-top:2px}` +
  `.legal-main .vf-hl{margin:0;font-size:18px;line-height:1.4;font-weight:600;color:var(--text);text-wrap:pretty}` +
  `.vf-hl>span{display:block}` +
  `.legal-main .vf-org{margin:10px 0 0;font-size:14.5px;line-height:1.5;color:var(--text-2);overflow-wrap:anywhere}` +
  `.legal-main .vf-next{margin:8px 0 0;font-size:15px;line-height:1.5;color:var(--text)}` +
  `.legal-main .vf-res a,.vf-lnk{color:var(--text);text-decoration:underline;text-underline-offset:2px;text-decoration-color:color-mix(in srgb,var(--text-2) 60%,transparent)}` +
  `.legal-main .vf-res a:hover{background:none}` +
  `.vf-lnk{font:inherit;background:none;border:0;padding:0;cursor:pointer}` +
  `.vf-hd .vf-lnk{display:inline-block;margin-top:10px;font-size:14.5px}` +
  `.vf-res[data-state="level2"]{border-color:color-mix(in srgb,var(--accent) 60%,var(--border))}` +
  `.vf-res[data-state="level2"] .vf-hd{background:var(--accent-soft);color:var(--accent-strong)}` +
  `.vf-res[data-state="level2"] .vf-hl{color:var(--accent-strong)}` +
  `.vf-res:is([data-state="tampered"],[data-state="modified"]){border-color:var(--coral)}` +
  `.vf-res:is([data-state="tampered"],[data-state="modified"]) .vf-hd{background:color-mix(in srgb,var(--coral) 9%,var(--surface));color:var(--coral)}` +
  `.vf-res:not([data-state^="level"]):not([data-state="tampered"]):not([data-state="modified"]) .vf-hl{font-size:17px;font-weight:500}` +
  `.vf-dns{padding:0 24px 20px}` +
  `.vf-btn{display:inline-flex;align-items:center;gap:9px;min-height:44px;padding:10px 16px;border-radius:9px;border:1px solid color-mix(in srgb,var(--text-2) 45%,var(--border));background:var(--surface);color:var(--text);font:600 15px/1.35 'DM Sans',system-ui,sans-serif;text-align:left;cursor:pointer;transition:border-color .16s ease,background-color .16s ease}` +
  `.vf-btn::before{content:"";flex:none;width:17px;height:17px;background:var(--text-2);-webkit-mask:url(/assets/icons/globe.svg) center/contain no-repeat;mask:url(/assets/icons/globe.svg) center/contain no-repeat}` +
  `.vf-btn:hover{border-color:var(--text-2);background:var(--surface-2)}` +
  `[data-ps-root] .vf-btn:focus-visible,[data-ps-root] .vf-res :is(a,button):focus-visible{outline:2px solid var(--text);outline-offset:2px}` +
  `.vf-btn span{overflow-wrap:anywhere}` +
  `.vf-btn[disabled]{cursor:progress;color:var(--text-2);background:var(--surface-2)}` +
  `.legal-main .vf-note{margin:10px 0 0;max-width:64ch;font-size:13.5px;line-height:1.55;color:var(--text-2)}` +
  `.vf-note code{font-family:'JetBrains Mono',monospace;font-size:12.5px;padding:1px 5px;border-radius:5px;background:var(--surface-2);color:var(--text);overflow-wrap:anywhere}` +
  `.legal-main .vf-bind{margin:14px 0 0;font-size:14.5px;line-height:1.5;color:var(--text);overflow-wrap:anywhere}` +
  `.vf-bind:focus{outline:none}` +
  `.vf-bind[data-binding="bound"]{color:var(--accent-strong);font-weight:600}` +
  `.legal-main .vf-alert{display:flex;gap:10px;align-items:flex-start;padding:12px 14px;border-radius:10px;background:color-mix(in srgb,var(--coral) 9%,var(--surface));border:1px solid color-mix(in srgb,var(--coral) 35%,var(--border))}` +
  `.legal-main .vf-alert p{margin:0;color:var(--text)}` +
  `.vf-alert [data-ico]{flex:none;width:18px;height:18px;margin-top:2px;color:var(--coral)}` +
  `.vf-alert strong{font-weight:600}` +
  `.legal-main .vf-dns>.vf-bind:first-child{margin-top:0}` +
  `.vf-fig{padding:0 24px 18px}` +
  `.legal-main .vf-fig .vf-gh{margin-top:2px}` +
  `.vf-det{border-top:1px solid var(--border);padding:4px 24px 18px}` +
  `.legal-main .vf-gh{margin:22px 0 4px;font-size:12px;font-weight:700;letter-spacing:.05em;text-transform:uppercase;color:var(--text-2)}` +
  `.legal-main .vf-gnote{margin:0 0 6px;font-size:13.5px;line-height:1.5}` +
  `.vf-dl{display:grid;grid-template-columns:minmax(0,16rem) minmax(0,1fr);column-gap:20px;margin:0}` +
  `.vf-dl>div{display:contents}` +
  `.vf-dl>div>dt,.vf-dl>div>dd{padding:9px 0;border-top:1px solid color-mix(in srgb,var(--border) 75%,transparent)}` +
  `.vf-dl>div:first-child>*{border-top:0}` +
  `.vf-dl dt{font-size:14px;line-height:1.45;color:var(--text-2)}` +
  `.vf-dl dd{margin:0;font-size:15px;line-height:1.45;color:var(--text);overflow-wrap:anywhere}` +
  `.vf-n{font-family:'JetBrains Mono',monospace;font-variant-numeric:tabular-nums}` +
  `.vf-dl dd.vf-big{font-size:21px;font-weight:600;letter-spacing:-.01em;line-height:1.3}` +
  `.vf-m{font-family:'JetBrains Mono',monospace;font-size:12.5px}` +
  `.vf-free{font-style:italic}` +
  `.vf-dl dd.vf-big-list{font-size:16px}` +
  `.vf-rates{display:grid;grid-template-columns:max-content max-content;gap:4px 18px;margin:0;padding:0;list-style:none}` +
  `.vf-rates li{display:contents}` +
  `.vf-rates span,.vf-rates b{color:var(--text)}` +
  `.vf-rates b{font-family:'JetBrains Mono',monospace;font-weight:500;font-variant-numeric:tabular-nums;text-align:right}` +
  `.vf-rates:not([data-plain]) li:last-child>*{font-weight:600}` +
  `.legal-main .vf-cnote{margin:6px 0 4px;padding:11px 14px;border-radius:10px;background:var(--surface-2);font-size:14px;line-height:1.55;color:var(--text-2)}` +
  `.vf-ft{display:flex;flex-wrap:wrap;gap:8px 26px;padding:14px 24px;border-top:1px solid var(--border);font-size:14px}` +
  `.vf-drop{position:relative;display:flex;flex-direction:column;align-items:center;gap:8px;margin-top:30px;padding:34px 24px;text-align:center;border:1.5px dashed color-mix(in srgb,var(--text-2) 45%,var(--border));border-radius:14px;background:var(--surface);cursor:pointer;transition:border-color .16s ease,background-color .16s ease}` +
  `.vf-drop:hover,.vf-drop[data-over]{border-color:var(--accent);background:color-mix(in srgb,var(--accent-soft) 45%,var(--surface))}` +
  `.vf-drop[data-over]{border-style:solid}` +
  `.vf-drop:focus-within{outline:2px solid var(--accent);outline-offset:2px}` +
  `.vf-drop input{position:absolute;width:1px;height:1px;opacity:0}` +
  `.vf-drop [data-ico]{width:26px;height:26px;color:var(--text-2)}` +
  `.vf-drop[data-over] [data-ico]{color:var(--accent-strong)}` +
  `.vf-drop strong{font-size:15.5px;font-weight:600;color:var(--text)}` +
  `.vf-drop strong u{text-underline-offset:2px}` +
  `.vf-drop small{display:block;margin-top:8px;font-size:13.5px;color:var(--text-2)}` +
  `.vf-drop[data-compact]{flex-direction:row;align-items:center;gap:14px;margin-top:20px;padding:14px 18px;text-align:left}` +
  `.vf-drop[data-compact] [data-ico]{flex:none;width:20px;height:20px}` +
  `.vf-drop[data-compact] strong{font-size:14.5px;font-weight:500}` +
  `.vf-drop[data-compact] small{display:none}` +
  `.vf-lim{margin-top:30px;border-top:1px solid var(--border);padding-top:16px;scroll-margin-top:84px}` +
  `.vf-lim>summary{display:inline-flex;align-items:center;gap:6px;min-height:32px;list-style:none;cursor:pointer;font-size:15px;font-weight:600;color:var(--text)}` +
  `.vf-lim>summary::-webkit-details-marker{display:none}` +
  `.vf-lim>summary [data-ico]{width:16px;height:16px;color:var(--text-2);transition:transform .2s ease}` +
  `.vf-lim[open]>summary [data-ico]{transform:rotate(90deg)}` +
  `.legal-main .vf-lim p{margin:10px 0 0}` +
  `.vf-lim .vf-meth{display:inline-block;margin-top:12px;font-size:14.5px}` +
  `@media(prefers-reduced-motion:reduce){.vf-btn,.vf-drop,.vf-lim>summary [data-ico]{transition:none}}` +
  `@media(max-width:560px){.legal-main{padding:36px 16px 64px}.vf-hd{padding:18px 18px 16px;gap:12px}.vf-dns,.vf-det{padding-left:18px;padding-right:18px}` +
  `.vf-btn{display:flex;width:100%;align-items:flex-start}.vf-btn::before{margin-top:2px}.vf-ft{padding:14px 18px}` +
  `.vf-dl{grid-template-columns:minmax(0,1fr)}.vf-dl>div>dt{padding-bottom:0}.vf-dl>div>dd{padding-top:2px;border-top:0}}</style>`;

writeFileSync(join(SITE, '404.html'), page('en', '/fr/404', NF.en.title, NF.en.desc, notFound('en'), { robots: 'noindex,follow', head: nfCss + frRedirect }));
writeFileSync(join(SITE, 'fr', '404.html'), page('fr', '/404', NF.fr.title, NF.fr.desc, notFound('fr'), { robots: 'noindex,follow', head: nfCss }));
writeFileSync(join(SITE, 'mentions-legales.html'), page('fr', '/legal-notice', 'Mentions légales', 'Mentions légales du site Perf Sentinel : éditeur, directeur de la publication et hébergeur.', mentionsFR));
writeFileSync(join(SITE, 'legal-notice.html'), page('en', '/mentions-legales', 'Legal notice', 'Legal notice for the Perf Sentinel site: publisher, publication director and host.', mentionsEN));
writeFileSync(join(SITE, 'confidentialite.html'), page('fr', '/privacy-policy', 'Politique de confidentialité', 'Politique de confidentialité du site Perf Sentinel : aucun cookie, aucun traceur, aucune collecte de données.', confidFR));
writeFileSync(join(SITE, 'privacy-policy.html'), page('en', '/confidentialite', 'Privacy policy', 'Privacy policy for the Perf Sentinel site: no cookies, no trackers, no data collection.', confidEN));
writeFileSync(join(SITE, 'verifier.html'), page('en', '/fr/verifier', COPY.en.title, COPY.en.desc, verifierMain('en'), { head: verifierCss + verifierRedirect }));
writeFileSync(join(SITE, 'fr', 'verifier.html'), page('fr', '/verifier', COPY.fr.title, COPY.fr.desc, verifierMain('fr'), { head: verifierCss }));
console.log('wrote mentions-legales + legal-notice + confidentialite + privacy-policy + 404 + verifier (en, fr)');
