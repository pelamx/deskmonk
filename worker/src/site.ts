//! deskmonk.app's pages in the visitor's language. The HTML in ../docs is English; elements marked
//! data-i18n="key" (inner HTML) or data-i18n-content="key" (a meta tag's content) are replaced here from i18n.ts,
//! data-price="monthly|yearly" gets the price, and the language menu is filled in. Runs before the static assets
//! only for the paths in wrangler.toml's run_worker_first.

import { LANG_NAMES, LANGS, RTL, USD, countryOf, money, pickLang, word, type Lang } from "./i18n";
import { esc } from "./admin";
import { js } from "./pages";

const PAGES = new Set(["/", "/index.html", "/pricing", "/pricing.html"]);

/** The platforms on the download section. Each one's machine owns docs/update/<platform>.json (signed, see
 *  packaging/publish-update.sh); nothing here or in the HTML names a version or file, so one platform's release
 *  never touches another's. */
export const PLATFORMS = ["macos", "windows-x86_64", "linux-x86_64"] as const;
export type Platform = (typeof PLATFORMS)[number];

/** One platform's release: `download` is the website file; `url` and `sha256`, when present, the update file the apps fetch. */
export interface Release {
  version: string;
  download: string;
  size: number;
  url?: string;
  sha256?: string;
}

/** The platform a download path belongs to: "/downloads/deskMonk-0.5.3-macos.dmg" → "macos". */
export function platformOf(path: string): Platform | null {
  const p = path.toLowerCase();
  if (p.includes("macos")) return "macos";
  if (p.includes("windows")) return "windows-x86_64";
  if (p.includes("linux") && !p.includes("aarch64") && !p.includes("arm64")) return "linux-x86_64";
  return null;
}

/** "v0.5.7 · DMG · 16 MB" */
export function downloadLabel(r: Release, platform: Platform): string {
  const ext = r.download.split(".").pop() ?? "";
  const kind = ext === "AppImage" ? "AppImage" : ext.toUpperCase();
  const arch = platform === "linux-x86_64" ? " · x86_64" : "";
  return `v${r.version} · ${kind}${arch} · ${Math.max(1, Math.round(r.size / 1048576))} MB`;
}

async function releases(assets: Fetcher, origin: string): Promise<Partial<Record<Platform, Release>>> {
  const out: Partial<Record<Platform, Release>> = {};
  await Promise.all(
    PLATFORMS.map(async (p) => {
      const res = await assets.fetch(`${origin}/update/${p}.json`);
      if (!res.ok) return;
      const r = ((await res.json()) as Record<string, Release>)[p];
      if (r && typeof r.version === "string" && typeof r.download === "string" && r.download.startsWith("/downloads/")) out[p] = r;
    }),
  );
  return out;
}

// The Google tag and the conversions live in this container. Its two click events come from docs/site.js.
const GTM_ID = "GTM-KK2TWW72";
// No consent banner, so no Tag Manager where advertising cookies need prior consent: the EU, EEA, UK, Switzerland and Türkiye (where the controller is).
const NO_ADS = new Set("AT BE BG HR CY CZ DK EE FI FR DE GR HU IE IT LV LT LU MT NL PL PT RO SK SI ES SE IS LI NO GB CH TR".split(" "));

const LOAD_GTM = `window.dataLayer = window.dataLayer || [];
  dataLayer.push({ "gtm.start": Date.now(), event: "gtm.js" });
  var s = document.createElement("script");
  s.async = true;
  s.src = "https://www.googletagmanager.com/gtm.js?id=${GTM_ID}";
  document.head.appendChild(s);`;

/** Google Tag Manager for /ads.js; the browser's Global Privacy Control stops it everywhere. Where advertising cookies need
 *  prior consent (and when the country is unknown) it loads only after Allow on a banner; the choice is the "ads" cookie,
 *  and any [data-cookies] link (the privacy page) clears it to ask again. */
export function adsScript(req: Request): Response {
  const country = countryOf(req);
  const lang = pickLang(req);
  const w = (k: string) => js(word(lang, `consent.${k}`));
  const body = country && !NO_ADS.has(country) ? `if (!navigator.globalPrivacyControl) {
  ${LOAD_GTM}
}
` : `(function () {
  function load() {
    window.dataLayer = window.dataLayer || [];
    function gtag() { dataLayer.push(arguments); }
    gtag("consent", "default", { ad_storage: "granted", ad_user_data: "granted", ad_personalization: "granted", analytics_storage: "granted" });
    ${LOAD_GTM}
  }
  function keep(v) { document.cookie = "ads=" + v + "; Path=/; Max-Age=" + (v ? 15552000 : 0) + "; SameSite=Lax; Secure"; }
  document.addEventListener("click", function (e) {
    if (e.target.closest && e.target.closest("[data-cookies]")) { e.preventDefault(); keep(""); location.reload(); }
  });
  if (navigator.globalPrivacyControl) return;
  var c = /(?:^|; )ads=([01])(?:;|$)/.exec(document.cookie);
  if (c) { if (c[1] === "1") load(); return; }
  function ask() {
    var box = document.createElement("div");
    box.className = "consent";
    box.setAttribute("role", "dialog");
    box.setAttribute("aria-label", "Cookies");
    var p = document.createElement("p");
    p.textContent = ${w("text")} + " ";
    var more = document.createElement("a");
    more.href = "/privacy#this-website";
    more.textContent = ${w("more")};
    p.appendChild(more);
    box.appendChild(p);
    [["0", ${w("decline")}], ["1", ${w("accept")}]].forEach(function (b) {
      var btn = document.createElement("button");
      btn.type = "button";
      btn.textContent = b[1];
      btn.onclick = function () { keep(b[0]); box.remove(); if (b[0] === "1") load(); };
      box.appendChild(btn);
    });
    document.body.appendChild(box);
  }
  if (document.body) ask(); else document.addEventListener("DOMContentLoaded", ask);
})();
`;
  return new Response(body, { headers: { "Content-Type": "text/javascript; charset=utf-8", "Cache-Control": "private, no-cache", Vary: "Cookie, Accept-Language" } });
}

/** Where a visitor came from: the link's ?from= (our download buttons) or utm_source, a Google Ads click, else the
 *  referring site's host ("google" for any Google search), else "direct". Lowercase, at most 60 plain characters. */
export function sourceOf(url: URL, referer: string | null): string {
  const tag = url.searchParams.get("from") ?? url.searchParams.get("utm_source") ?? (url.searchParams.has("gclid") ? "google ads" : null);
  let host = "";
  try { host = new URL(referer ?? "").hostname.replace(/^www\./, ""); } catch { /* no or bad Referer */ }
  const raw = tag ?? (!host || host === url.hostname ? "direct" : /^google\.[a-z.]+$/.test(host) ? "google" : host);
  return raw.toLowerCase().replace(/[^a-z0-9._ -]/g, "").slice(0, 60) || "direct";
}

/** Records a website visit or download (visits table) with its source; the IP address is not kept. */
export type Track = (kind: "visit" | "download", source: string, platform: Platform | null) => void;

// Crawlers and link previews fetch "/" too; they aren't visitors.
const BOT = /bot|crawl|spider|slurp|preview|fetch|curl|wget|python|go-http|headless|monitor/i;

export async function handleSite(req: Request, assets: Fetcher, path: string, track: Track = () => {}): Promise<Response | null> {
  if (path === "/ads.js") return adsScript(req);
  const url = new URL(req.url);
  const human = req.method === "GET" && !BOT.test(req.headers.get("User-Agent") ?? "");
  // /download/macos, and any download file that is no longer there (an old version's link): the platform's current file.
  // The worker only sees /downloads/ paths that have no file, as the static assets answer the rest first.
  if (path.startsWith("/download/") || path.startsWith("/downloads/")) {
    const platform = path.startsWith("/download/") ? (PLATFORMS as readonly string[]).includes(path.slice(10)) ? (path.slice(10) as Platform) : null : platformOf(path);
    const r = platform && (await releases(assets, new URL(req.url).origin))[platform];
    if (r && human) track("download", sourceOf(url, req.headers.get("Referer")), platform);
    return r ? Response.redirect(new URL(r.download, req.url).toString(), 302) : new Response("not found", { status: 404 });
  }
  if (!PAGES.has(path) || (req.method !== "GET" && req.method !== "HEAD")) return null;
  // Without the browser's If-None-Match: the same URL now has a body per language, so a 304 could show the wrong one.
  const res = await assets.fetch(new Request(req.url, { method: req.method }));
  if (!res.ok || !(res.headers.get("Content-Type") ?? "").includes("text/html")) return res;
  const lang = pickLang(req);
  const source = sourceOf(url, req.headers.get("Referer"));
  // A click from one of our own pages to another is not a new visit.
  // shortcut: that page's download buttons then say "direct", not where the visitor first came from; carry it in a cookie if that skews the numbers.
  if (human && !(req.headers.get("Referer") ?? "").startsWith(url.origin)) track("visit", source, null);
  const out = withDownloads(translate(res, lang, url), await releases(assets, url.origin), source);
  const headers = new Headers(out.headers);
  headers.delete("ETag");
  headers.set("Cache-Control", "no-cache");
  headers.set("Vary", "Cookie, Accept-Language");
  headers.set("Content-Language", lang);
  if (new URL(req.url).searchParams.has("lang")) headers.append("Set-Cookie", `lang=${lang}; Path=/; Max-Age=31536000; SameSite=Lax; Secure`);
  return new Response(out.body, { status: out.status, headers });
}

/** Points each a[data-download="<platform>"] at /download/<platform> (which counts the download with its source and
 *  sends the platform's current file) and writes its version and size in its <small>. */
export function withDownloads(res: Response, rel: Partial<Record<Platform, Release>>, source: string): Response {
  let current: Platform | null = null;
  return new HTMLRewriter()
    .on("a[data-download]", {
      element: (el) => {
        const p = el.getAttribute("data-download") as Platform;
        const r = rel[p];
        current = r ? p : null;
        if (r) el.setAttribute("href", `/download/${p}?from=${encodeURIComponent(source)}`);
      },
    })
    .on("a[data-download] small", { element: (el) => { if (current) el.setInnerContent(downloadLabel(rel[current]!, current)); } })
    .transform(res);
}

export function translate(res: Response, lang: Lang, url: URL): Response {
  const alternates = LANGS.map((code) => `<link rel="alternate" hreflang="${code}" href="${url.origin}${url.pathname}?lang=${code}">`).join("");
  const menu = LANGS.map((code) => `<option value="${code}"${code === lang ? " selected" : ""}>${esc(LANG_NAMES[code])}</option>`).join("");
  return new HTMLRewriter()
    .on("html", { element: (el) => { el.setAttribute("lang", lang); if (RTL.has(lang)) el.setAttribute("dir", "rtl"); } })
    .on("head", { element: (el) => void el.append(`${alternates}<link rel="alternate" hreflang="x-default" href="${url.origin}${url.pathname}">`, { html: true }) })
    // A key missing from en.json keeps the HTML's own English.
    .on("[data-i18n]", { element: (el) => { const w = word(lang, el.getAttribute("data-i18n") ?? ""); if (w) el.setInnerContent(w, { html: true }); } })
    .on("[data-i18n-content]", { element: (el) => { const w = word(lang, el.getAttribute("data-i18n-content") ?? ""); if (w) el.setAttribute("content", w); } })
    .on("[data-price]", { element: (el) => void el.setInnerContent(money(lang, ({ free: 0, monthly: USD.monthly, yearly: USD.yearly } as Record<string, number>)[el.getAttribute("data-price") ?? ""] ?? 0)) })
    .on("select.langs", { element: (el) => void el.setInnerContent(menu, { html: true }) })
    .on('a[href^="/buy"], a[href^="/account"]', {
      element: (el) => {
        const href = el.getAttribute("href") ?? "";
        el.setAttribute("href", `${href}${href.includes("?") ? "&" : "?"}lang=${lang}`);
      },
    })
    .transform(res);
}
