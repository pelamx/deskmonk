//! The pages a buyer sees, in the visitor's language (i18n.ts: ?lang=, the "lang" cookie the website also sets,
//! the visitor's country, then Accept-Language). Same look as deskmonk.app: the monk logo's palette, the site's logo and legal links.
//! GET  /buy?plan=monthly|yearly[&install=<id>]   plan cards with local prices, email, then Paddle's overlay
//! GET  /welcome?ref=<ref>                        after paying: waits for the webhook, then shows the license key
//! GET  /account, POST /account {key}             a license's plan, status, end date and devices
//! POST /account/portal {key}                     → Paddle's page to cancel or change the card
//! GET  /plx?ref=<ref>                            the plx (USDC) payment page is in plx.ts

import { esc } from "./admin";
import { LANG_NAMES, LANGS, RTL, USD, money, words, type Lang } from "./i18n";

export { pickLang } from "./i18n";

/** The buyer pages' texts ("pay.*" keys of ../i18n/<code>.json) in the shape the pages use. */
export function t(lang: Lang) {
  const w = words(lang, "pay.");
  const nested = (prefix: string): Record<string, string> =>
    Object.fromEntries(Object.entries(w).filter(([k]) => k.startsWith(prefix)).map(([k, v]) => [k.slice(prefix.length), v]));
  return { ...w, st: nested("st."), planName: nested("planName.") } as Record<string, string> & {
    st: Record<string, string>;
    planName: Record<string, string>;
  };
}
export type Strings = ReturnType<typeof t>;

/** Paddle checkout's own languages (its `locale`); for the others Paddle picks from the browser. */
const PADDLE_LOCALE: Record<string, string> = {
  en: "en", tr: "tr", es: "es", de: "de", fr: "fr", it: "it", pt: "pt", nl: "nl", pl: "pl", ru: "ru", da: "da", sv: "sv",
  nb: "no", ja: "ja", ko: "ko", ar: "ar", zh: "zh-Hans", "zh-TW": "zh-Hant",
};

/** For values put inside a <script>: JSON with "<" escaped so nothing can close the tag. */
export const js = (v: unknown): string => JSON.stringify(v).replace(/</g, "\\u003c");

const STYLE = `
:root{color-scheme:dark;--bg:#110e20;--soft:#181431;--card:#1b1735;--line:#2e2752;--text:#f5f1ea;--muted:#b0a9c8;--brand:#6c63ff;--brand-2:#8178ff;--accent:#f28c28;--gold:#ffd23f}
*{box-sizing:border-box}
body{margin:0;padding:40px 16px;color:var(--text);display:flex;flex-direction:column;align-items:center;position:relative;
  font:15px/1.6 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;
  background:radial-gradient(1100px 560px at 85% -8%,rgba(108,99,255,.22),transparent 60%),radial-gradient(900px 520px at 0% 6%,rgba(242,140,40,.10),transparent 55%),var(--bg);background-attachment:fixed}
@media(max-width:560px){body{padding-top:56px}}
.sheet{width:100%;max-width:620px}
.brandbar{display:flex;align-items:center;gap:10px;margin:0 0 26px;font-weight:800;font-size:1.1rem;color:var(--text);text-decoration:none}
.brandbar img{width:34px;height:34px;border-radius:9px}
h1{font-size:28px;line-height:1.15;margin:0 0 8px;font-weight:800;letter-spacing:-.4px}
h2{font-size:15px;margin:28px 0 10px}
p{margin:0 0 14px;color:var(--muted)}
.muted{color:var(--muted);font-size:13px}
a{color:#a49dff}
.tiers{display:grid;gap:12px;grid-template-columns:1fr 1fr;margin:18px 0 6px}
.tier{border-inline-start:2px solid var(--line);padding:2px 0;padding-inline-start:14px;font-size:13px;color:var(--muted)}
.tier .name{color:var(--text);font-weight:600;margin-bottom:3px}
.tier.pro{border-inline-start-color:var(--accent)}.tier.pro .name{color:var(--gold)}
.paycard{background:linear-gradient(180deg,rgba(35,29,71,.92),rgba(27,23,53,.92));border:1px solid var(--line);border-radius:18px;padding:6px 22px 22px;margin:22px 0 0;box-shadow:0 24px 60px rgba(0,0,0,.4)}
.paycard-top{display:flex;align-items:center;justify-content:center;gap:7px;padding:14px 0 2px;font-size:12.5px;color:var(--muted)}
.paycard-top svg{color:var(--accent)}
.plans{display:grid;gap:12px;grid-template-columns:1fr 1fr;margin:16px 0 18px}
.plan{border:1px solid var(--line);border-radius:14px;padding:16px;cursor:pointer;background:rgba(17,14,32,.55);display:block;position:relative;transition:border-color .15s,box-shadow .15s}
.plan:hover{border-color:#463c7a}
.plan.on{border-color:var(--brand);background:var(--soft);box-shadow:0 0 0 1px var(--brand),0 10px 30px rgba(108,99,255,.25)}
.plan input{position:absolute;opacity:0;pointer-events:none}
.plan .name{font-size:13px;color:var(--muted)}
.plan .price{font-size:26px;font-weight:700;margin:4px 0 2px;letter-spacing:-.5px}
.plan .note{font-size:12px;color:var(--accent)}
.trust{display:flex;gap:8px 18px;flex-wrap:wrap;justify-content:center;margin:16px 0 0;font-size:12.5px;color:var(--muted)}
.trust span{display:inline-flex;align-items:center;gap:6px}
.trust span::before{content:"";width:5px;height:5px;border-radius:50%;background:var(--accent)}
.fineprint{font-size:12.5px;margin:16px 2px 0;text-align:center}
label.field{display:block;font-size:13px;color:var(--muted);margin:0 0 6px}
input[type=text],input[type=email]{width:100%;padding:12px 14px;border-radius:10px;border:1px solid var(--line);background:rgba(17,14,32,.6);color:var(--text);font-size:15px;font-family:inherit}
input:focus{outline:none;border-color:var(--brand);box-shadow:0 0 0 3px rgba(108,99,255,.28)}
button{width:100%;padding:13px 18px;border-radius:999px;border:0;cursor:pointer;background:linear-gradient(135deg,var(--brand),var(--brand-2));color:#fff;font-size:15px;font-weight:600;font-family:inherit;margin-top:14px;box-shadow:0 10px 30px rgba(108,99,255,.38);transition:transform .15s}
button:hover:not(:disabled){transform:translateY(-2px)}
button:disabled{opacity:.55;cursor:default}
button.quiet{background:transparent;border:1px solid var(--line);color:var(--text);box-shadow:none}
.key{font:20px/1.4 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;letter-spacing:.06em;background:rgba(17,14,32,.6);border:1px solid var(--accent);border-radius:12px;padding:16px;text-align:center;user-select:all;margin:8px 0 14px;word-break:break-all}
.facts{background:var(--card);border:1px solid var(--line);border-radius:14px;padding:6px 18px;margin:16px 0}
.facts div{display:flex;justify-content:space-between;gap:12px;padding:9px 0;border-bottom:1px solid var(--line)}
.facts div:last-child{border:0}.facts span{color:var(--muted)}
.ok{color:var(--accent)}.warn{color:var(--gold)}.err{color:#ff9696}
hr{border:0;border-top:1px solid var(--line);margin:26px 0}
ol{color:var(--muted);padding-inline-start:20px}li{margin-bottom:6px}
.mascot{display:block;width:96px;margin:0 auto 6px}
.langs{position:absolute;top:14px;inset-inline-end:18px}
.langs select{font:inherit;font-size:13px;color:var(--text);background:var(--card);border:1px solid var(--line);border-radius:999px;padding:4px 10px;max-width:160px}
footer{max-width:620px;margin:30px auto 0;text-align:center;font-size:.82rem;display:flex;gap:16px;justify-content:center;flex-wrap:wrap}
footer a{color:var(--muted);text-decoration:none}
@media(max-width:520px){.paycard{padding:4px 16px 18px}.plans,.tiers{grid-template-columns:1fr}}
@media(prefers-reduced-motion:reduce){button,.plan{transition:none}button:hover:not(:disabled){transform:none}}`;

const LOCK = `<svg viewBox="0 0 24 24" aria-hidden="true" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="4" y="10.5" width="16" height="10" rx="2.5"/><path d="M8 10.5V7a4 4 0 0 1 8 0v3.5"/></svg>`;

function withLang(url: URL, lang: Lang): string {
  const u = new URL(url);
  u.searchParams.set("lang", lang);
  return u.pathname + u.search;
}

export function page(req: Request, lang: Lang, title: string, body: string, opts: { head?: string; status?: number } = {}): Response {
  const s = t(lang);
  const url = new URL(req.url);
  const switcher = `<select aria-label="Language" onchange="location.href=this.value">${LANGS.map(
    (code) => `<option value="${esc(withLang(url, code))}"${code === lang ? " selected" : ""}>${esc(LANG_NAMES[code])}</option>`,
  ).join("")}</select>`;
  const html = `<!doctype html><html lang="${lang}"${RTL.has(lang) ? ' dir="rtl"' : ""}><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<link rel="icon" href="/logo.svg"><title>${esc(title)}</title><style>${STYLE}</style>${opts.head ?? ""}</head>
<body><nav class="langs">${switcher}</nav><div class="sheet">
<a class="brandbar" href="/?lang=${lang}"><img src="/logo.svg" alt="">deskMonk</a>${body}</div>
<footer><a href="/?lang=${lang}">${s.home}</a><a href="/terms">${s.terms}</a><a href="/privacy">${s.privacy}</a><a href="/refunds">${s.refunds}</a><a href="mailto:support@deskmonk.app">support@deskmonk.app</a></footer>
</body></html>`;
  // Not HttpOnly: the website's language switcher (docs/site.js) writes the same cookie.
  return new Response(html, {
    status: opts.status ?? 200,
    headers: { "Content-Type": "text/html; charset=utf-8", "Set-Cookie": `lang=${lang}; Path=/; Max-Age=31536000; SameSite=Lax; Secure` },
  });
}

export interface BuyOptions {
  configured: boolean;
  plan: "monthly" | "yearly";
  install: string;
  token: string;
  sandbox: boolean;
  prices: { monthly: string; yearly: string };
  /** The "Pay with plx" button (USDC on Solana, plx.ts). */
  plx: boolean;
  /** A plx license key to extend ("Renew" on /account), or "". */
  key: string;
  /** A real two-letter country from Cloudflare, or null (XX = unknown, T1 = Tor are not countries). */
  country: string | null;
}

export function buyPage(req: Request, lang: Lang, o: BuyOptions): Response {
  const s = t(lang);
  const tiers = `<p>${s.intro}</p><div class="tiers">
    <div class="tier"><div class="name">${s.freeName}</div>${s.freeText}</div>
    <div class="tier pro"><div class="name">${s.proName}</div>${s.proText}</div></div>`;
  if (!o.configured) {
    return page(req, lang, s.proTitle, `<h1>${s.proTitle}</h1>${tiers}<hr><p class="warn"><b>${s.soonTitle}</b></p><p>${s.soon}</p>`, { status: 503 });
  }
  const card = (cycle: "monthly" | "yearly", price: string, note: string) => `
    <label class="plan${o.plan === cycle ? " on" : ""}"><input type="radio" name="cycle" value="${cycle}"${o.plan === cycle ? " checked" : ""}>
      <div class="name">${cycle === "yearly" ? s.yearly : s.monthly}</div><div class="price" data-cycle="${cycle}">${price}</div><div class="note">${note}</div></label>`;
  const origin = new URL(req.url).origin;
  const body = `<h1>${s.proTitle}</h1>${tiers}
  <section class="paycard" aria-label="${esc(s.proTitle)}">
    <header class="paycard-top">${LOCK}<span>${s.secure}</span></header>
    <div class="plans">${card("monthly", money(lang, USD.monthly), s.monthlyNote)}${card("yearly", money(lang, USD.yearly), s.monthlyNote)}</div>
    ${o.install ? `<p class="ok" style="font-size:13px">${s.fromApp}</p>` : ""}
    <label class="field" for="email">${s.email}</label>
    <input id="email" type="email" required autocomplete="email" placeholder="${esc(s.emailPh)}">
    <button id="pay" type="button">${s.pay}</button>
    ${o.plx ? `<button id="plx" type="button" class="quiet">${s.plxPay}</button><p class="muted" style="margin:8px 0 0;text-align:center">${s.plxNote}</p>` : ""}
    <div class="trust"><span>${s.trust1}</span><span>${s.trust2}</span><span>${s.trust3}</span></div>
  </section>
  <p class="muted fineprint">${s.fine}</p>
  <script src="https://cdn.paddle.com/paddle/v2/paddle.js"></script>
  <script>
  const c = ${js({ key: o.key, plx: `/plx?lang=${lang}`, token: o.token, sandbox: o.sandbox, prices: o.prices, install: o.install, country: o.country, locale: PADDLE_LOCALE[lang] ?? null, failed: s.failed, welcome: `${origin}/welcome?lang=${lang}&ref=` })};
  for (const input of document.querySelectorAll('.plan input')) input.addEventListener('change', () => {
    for (const p of document.querySelectorAll('.plan')) p.classList.toggle('on', p.contains(document.querySelector('.plan input:checked')));
  });
  // plx: a plain form post; the server makes the invoice and shows its QR code. The email is optional there.
  document.getElementById('plx')?.addEventListener('click', () => {
    const email = document.getElementById('email');
    if (email.value && !email.reportValidity()) return;
    const f = document.createElement('form');
    f.method = 'post'; f.action = c.plx;
    const fields = { cycle: document.querySelector('.plan input:checked').value, email: email.value, install: c.install, key: c.key };
    for (const [name, value] of Object.entries(fields)) { const i = document.createElement('input'); i.type = 'hidden'; i.name = name; i.value = value; f.append(i); }
    document.body.append(f); f.submit();
  });
  if (c.sandbox) Paddle.Environment.set('sandbox');
  Paddle.Initialize({ token: c.token });
  // Paddle's own totals in the buyer's currency, tax included; the dollar prices stay if this fails.
  Paddle.PricePreview({ items: [{ priceId: c.prices.monthly, quantity: 1 }, { priceId: c.prices.yearly, quantity: 1 }],
      ...(c.country ? { address: { countryCode: c.country } } : {}) })
    .then((r) => { for (const line of r.data.details.lineItems) {
      const cycle = line.price.id === c.prices.yearly ? 'yearly' : 'monthly';
      document.querySelector('.price[data-cycle="' + cycle + '"]').textContent = line.formattedTotals.total;
    } })
    .catch((e) => console.error('price preview failed', e));
  document.getElementById('pay').addEventListener('click', () => {
    const email = document.getElementById('email');
    if (!email.reportValidity()) return;
    const cycle = document.querySelector('.plan input:checked').value;
    // The order reference: the webhook gets it in custom_data, /welcome shows the key of the subscription carrying it.
    const ref = [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, '0')).join('');
    try {
      Paddle.Checkout.open({
        items: [{ priceId: c.prices[cycle], quantity: 1 }],
        customer: { email: email.value },
        customData: c.install ? { ref, install_id: c.install } : { ref },
        settings: { displayMode: 'overlay', variant: 'one-page', ...(c.locale ? { locale: c.locale } : {}), successUrl: c.welcome + ref },
      });
    } catch (e) { console.error(e); alert(c.failed); }
  });
  </script>`;
  return page(req, lang, s.proTitle, body);
}

export function welcomePage(req: Request, lang: Lang, ref: string): Response {
  const s = t(lang);
  const words = {
    received: s.received, activate: s.activate, steps: [s.step1, s.step2, s.step3], autoPro: s.autoPro,
    keep: s.keep, slow: s.slow, slowMore: s.slowMore, account: `/account?lang=${lang}`,
  };
  return page(
    req,
    lang,
    `${s.thanks} — ${s.proTitle}`,
    `<img class="mascot" src="/monk.svg" alt=""><h1 style="text-align:center">${s.thanks}</h1><div id="body"><p id="status" style="text-align:center">${s.confirming}</p></div>
  <script>
  const ref = ${js(ref)}, w = ${js(words)}, body = document.getElementById('body');
  const el = (tag, text, cls) => { const e = document.createElement(tag); if (text) e.textContent = text; if (cls) e.className = cls; return e; };
  let tries = 0;
  async function poll() {
    tries++;
    let order = null;
    try { order = await fetch('/api/order/' + ref).then((r) => r.json()); } catch {}
    if (order && order.status === 'paid') {
      body.replaceChildren(el('p', w.received, 'ok'), el('div', order.key, 'key'), el('h2', w.activate));
      const ol = el('ol'); for (const s of w.steps) ol.append(el('li', s)); body.append(ol);
      body.append(el('p', w.autoPro, 'muted'));
      const keep = el('p', w.keep, 'muted'); const a = el('a', 'deskmonk.app/account'); a.href = w.account; keep.append(a); body.append(keep);
      return;
    }
    if (tries > 45) {
      body.replaceChildren(el('p', w.slow, 'warn'), el('p', w.slowMore), el('div', ref.slice(0, 12), 'key'));
      return;
    }
    setTimeout(poll, 2000);
  }
  poll();
  </script>`,
  );
}

export interface AccountView {
  key: string;
  plan: string | null;
  status: string;
  periodEnd: number;
  devices: number;
  maxDevices: number;
  /** Paddle subscription (not one issued by hand): the portal can manage it. */
  paddle: boolean;
  /** Paid with plx: renewed by paying again. */
  plx: boolean;
}

export function accountPage(req: Request, lang: Lang, view: AccountView | null, error = ""): Response {
  const s = t(lang);
  const form = `<form method="post" action="/account?lang=${lang}">
    <label class="field" for="key">${s.key}</label>
    <input id="key" type="text" name="key" required placeholder="DM-XXXX-XXXX-XXXX" value="${esc(view?.key ?? "")}" autocapitalize="characters" spellcheck="false">
    <button type="submit"${view ? ' class="quiet"' : ""}>${s.lookUp}</button></form>`;
  if (view === null) {
    return page(
      req,
      lang,
      `${s.account} — deskMonk`,
      `<h1>${s.account}</h1><p>${s.accountIntro}</p>${error ? `<p class="err">${esc(error)}</p>` : ""}${form}<hr><p class="muted">${s.lost}</p>`,
      { status: error ? 404 : 200 },
    );
  }
  const now = Date.now() / 1000;
  const date = new Date(view.periodEnd * 1000).toLocaleDateString(lang === "en" ? "en-GB" : lang, { dateStyle: "long", timeZone: "UTC" });
  const live = view.periodEnd > now;
  const manage = view.paddle
    ? `<form method="post" action="/account/portal?lang=${lang}"><input type="hidden" name="key" value="${esc(view.key)}">
       <button type="submit">${s.manage}</button></form><p class="muted" style="margin-top:8px;text-align:center">${s.manageNote}</p>`
    : view.plx
      ? `<a href="/buy?lang=${lang}&plan=${view.plan === "yearly" ? "yearly" : "monthly"}&key=${encodeURIComponent(view.key)}"><button type="button">${s.plxRenew}</button></a>
         <p class="muted" style="margin-top:8px;text-align:center">${s.plxRenewNote}</p>`
      : "";
  return page(
    req,
    lang,
    `${s.account} — deskMonk`,
    `<h1>${s.account}</h1>${error ? `<p class="err">${esc(error)}</p>` : ""}
    <div class="key">${esc(view.key)}</div>
    <div class="facts">
      <div><span>${s.plan}</span><b>${esc(s.planName[view.plan ?? ""] ?? "Pro")}</b></div>
      <div><span>${s.status}</span><b class="${live ? "ok" : "warn"}">${esc(live ? (s.st[view.status] ?? view.status) : s.st.canceled)}</b></div>
      <div><span>${live ? s.until : s.ended}</span><b>${esc(date)}</b></div>
      <div><span>${s.devices}</span><b>${view.devices} / ${view.maxDevices}</b></div>
    </div>${manage}<hr>${form}<p class="muted">${s.lost}</p>`,
  );
}
