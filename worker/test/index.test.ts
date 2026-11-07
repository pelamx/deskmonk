import { expect, test } from "vitest";
import { compareVersions, esc, handleAdmin, hardwareHtml, humanSize, mobileCard, mobileLive, worldMap } from "../src/admin";
import { insert, parseEvent } from "../src/index";
import { handleLicense, inCidr, mayActivate, plxSignatureOk, refundLast, newKey, periodEnd, planOf, verifySignature } from "../src/license";
import { handleMail, keyMail, weekNumbers, weeklyText } from "../src/mail";
import { buyPage, js, pickLang } from "../src/pages";
import { LANGS, USD, money, word } from "../src/i18n";
import { adsScript, downloadLabel, platformOf, sourceOf } from "../src/site";

test("validates events the way server/ does", () => {
  const good = `{"install_id":"abc-123","version":"0.1.0","os":"linux","client":"tui","kind":"scan","files":10,"bytes":2048,"junk_bytes":2048}`;
  expect(parseEvent(good).sizes).toEqual([2048, 0, 2048, 0, 0]);
  expect(() => parseEvent(good.replace("abc-123", "<script>"))).toThrow();
  expect(() => parseEvent(good.replace('"scan"', '"drop"'))).toThrow();
  expect(() => parseEvent(good.replace("10", "-1"))).toThrow();
  expect(() => parseEvent(good.replace('"files":10', '"specs":[]'))).toThrow();

  const open = `{"install_id":"abc-123","version":"0.2.0","os":"macos","client":"gui","kind":"open",
    "specs":[{"kind":"İşlemci","brand":"Apple","name":"Apple M5 Pro <b>","detail":"15 çekirdek"}]}`;
  const ev = parseEvent(open);
  expect(() => parseEvent(open.replace("15 çekirdek", "a\\u0007"))).toThrow();
  const html = hardwareHtml("2026-10-29 10:00:00", ev.specs!);
  expect(html).toContain("[APPLE]");
  expect(html).toContain("Apple M5 Pro &lt;b&gt;");
  expect(html).not.toContain("Pro <b>");
});

test("events from 0.4: features, plan and store", () => {
  const vault = `{"install_id":"abc-123","version":"0.4.0","os":"macos","client":"gui","kind":"vault","detail":"encrypt","plan":"free","channel":"appstore"}`;
  expect(parseEvent(vault)).toMatchObject({ kind: "vault", detail: "encrypt", plan: "free", channel: "appstore" });
  expect(parseEvent(vault.replace(',"plan":"free","channel":"appstore"', "")).plan).toBeNull();
  expect(() => parseEvent(vault.replace('"free"', '"gold"'))).toThrow();
  expect(() => parseEvent(vault.replace('"appstore"', '"steam"'))).toThrow();
  expect(() => parseEvent(vault.replace('"encrypt"', '"<b>"'))).toThrow();
  expect(parseEvent(vault.replace('"vault"', '"ping"')).kind).toBe("ping");
  expect(parseEvent(vault.replace('"vault"', '"applock"').replace('"encrypt"', '"lock"')).kind).toBe("applock");
  expect(parseEvent(vault.replace('"appstore"', '"playstore"').replace('"macos"', '"android"'))).toMatchObject({ os: "android", channel: "playstore" });
});

const DAY_S = 86400;

test("mobile live: counts, status of each phone, and what a phone reports is escaped", () => {
  const t = 1_800_000_000;
  const html = mobileLive(
    {
      counts: [{ os: "ios", live: 2, hour: 3, today: 4, week: 5, month: 6 }, { os: "android", live: 1, hour: 1, today: 1, week: 1, month: 1 }],
      devices: [{ os: "ios", type: "Tablet", live: 7, hour: 8, today: 9, week: 10, month: 11 }, { os: "android", type: null, live: 12 }],
      recent: [
        { id: "aaaaaaaa-1", os: "ios", country: "TR", version: "1.0.0", plan: "pro", last_seen: t - 60, action: "clean", phone: "<b>iPhone</b>", type: "Phone" },
        { id: "bbbbbbbb-2", os: "android", country: null, version: "1.0.0", plan: "free", last_seen: t - 10 * DAY_S, action: null, phone: null },
      ],
    },
    t,
  );
  expect(html).toContain('<div class="v">3</div>'); // live now, both platforms
  expect(html).toContain("&lt;b&gt;iPhone");
  expect(html).not.toContain("<b>iPhone");
  expect(html).toMatch(/dot live"><\/span>Live<\/td><td><a href="\/admin\/user\/aaaaaaaa-1"/);
  expect(html).toMatch(/dot dormant"><\/span>Dormant/);
  expect(html).toContain("Cleaned");
  // All four devices are always listed, and installs that didn't say which they are stay visible.
  expect(html).toMatch(/iPhone[\s\S]*iPad<\/td><td class="num">7<\/td><td class="num">8<\/td><td class="num">9<\/td><td class="num">10<\/td><td class="num">11<\/td>[\s\S]*Android phone[\s\S]*Android tablet[\s\S]*Android, type not reported<\/td><td class="num">12</);
  expect(mobileLive({ counts: [], recent: [], devices: [] }, t)).toContain("No phone or tablet has reported yet");
});

test("mobile card: platforms add up, and no desktop numbers", () => {
  const t = 1_800_000_000;
  const html = mobileCard(
    {
      lifecycle: [{ os: "ios", type: "Phone", total: 7, new_today: 2, new_yesterday: 1, today: 2, month: 4, day_base: 4, day_back: 3, week_base: 2, week_back: 1, month_base: 0, month_back: 0, dormant: 1, gone: 2 },
        { os: "android", type: "Phone", total: 3, new_today: 1, new_yesterday: 0, today: 1, month: 2, day_base: 2, day_back: 1, week_base: 0, week_back: 0, month_base: 0, month_back: 0, dormant: 0, gone: 1 }],
      usage: [{ os: "ios", type: "Phone", active: 4, allowed: 4, scanned: 3, cleaned: 2, saw_pro: 2, bought: 1, pro: 1 }, { os: "android", type: "Phone", active: 2, allowed: 1, scanned: 1, cleaned: 1, saw_pro: 0, bought: 0, pro: 0 }],
      features: [{ os: "ios", kind: "clean", detail: "", n: 5, users: 2, bytes: 2048 }, { os: "ios", kind: "pro", detail: "topbar", n: 2, users: 2, bytes: 0 }, { os: "android", kind: "pro", detail: "clean_limit", n: 9, users: 1, bytes: 0 }],
      active: [], fresh: [], phones: [{ name: "Pixel <9>", n: 2 }], countries: [{ country: "TR", n: 3 }],
      devices: [{ os: "android", type: "Tablet", total: 21, new_today: 22, active: 23, pro: 24, scanned: 25, cleaned: 26, vaulted: 27, focused: 28, bought: 29 }, { os: "ios", type: "Phone", total: 4 }],
    },
    [{ version: "1.1.0", os: "ios", type: "Tablet", n: 5 }, { version: "1.0.0", os: "android", n: 2 }, { version: "0.3.0", os: "macos", n: 800 }],
    t,
  );
  expect(html).toContain("2.0 KB");
  expect(html).toContain("Pixel &lt;9&gt;");
  expect(html).toMatch(/Android tablet<\/td>(<td class="num">2\d<\/td>){9}/);
  expect(html).toContain("iPad");
  expect(html).not.toContain("0.3.0");
  expect(html.indexOf("1.1.0")).toBeLessThan(html.indexOf("1.0.0"));
  // Stickiness: 3 active today of 6 this month, across both platforms.
  expect(html).toContain('<div class="v">50%</div>');
  // The funnel's total adds both platforms; each step is measured against the one before.
  expect(html).toMatch(/Scanned<\/td><td class="num">3<\/td><td class="num">0<\/td><td class="num">1<\/td><td class="num">0<\/td><td class="num">4<\/td><td>80%/);
  // Versions split by device too, and an install that didn't say which it is keeps its own column.
  expect(html).toMatch(/1\.1\.0[\s\S]*?<td class="num"><span class="muted">0<\/span><\/td><td class="num">5<\/td>/);
  expect(html).toContain("Android, type not reported");
  // Most-used source first.
  expect(html.indexOf("Daily clean limit")).toBeLessThan(html.indexOf("Top bar button"));
});

test("only deskMonk's Paddle prices make a license", () => {
  const env = { DB: null as any, PADDLE_PRICE_MONTHLY: "pri_m", PADDLE_PRICE_YEARLY: "pri_y" };
  expect(planOf({ items: [{ price: { id: "pri_y" } }] }, env)).toBe("yearly");
  expect(planOf({ items: [{ price: { id: "pri_m" } }] }, env)).toBe("monthly");
  // Another product sold from the same Paddle account.
  expect(planOf({ items: [{ price: { id: "pri_other" } }] }, env)).toBeNull();
  expect(planOf({}, { DB: null as any })).toBeNull();
});

test("versions sort newest first", () => {
  expect(["0.3.0", "0.10.0", "0.9.1"].sort(compareVersions)).toEqual(["0.10.0", "0.9.1", "0.3.0"]);
});

test("formats", () => {
  expect(humanSize(2048)).toBe("2.0 KB");
  expect(humanSize(5)).toBe("5 B");
  expect(esc("<x>")).toBe("&lt;x&gt;");
});

test("Paddle webhook signatures", async () => {
  const body = '{"event_type":"subscription.created"}';
  const sign = async (ts: number, secret: string) => {
    const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(`${ts}:${body}`)));
    return [...mac].map((b) => b.toString(16).padStart(2, "0")).join("");
  };
  const now = 1_800_000_000;
  const good = `ts=${now};h1=${await sign(now, "whsec")}`;
  expect(await verifySignature(good, body, "whsec", now + 10)).toBe(true);
  expect(await verifySignature(`ts=${now};h1=00;h1=${await sign(now, "whsec")}`, body, "whsec", now)).toBe(true);
  expect(await verifySignature(good, body + " ", "whsec", now)).toBe(false);
  expect(await verifySignature(good, body, "other", now)).toBe(false);
  expect(await verifySignature(good, body, "whsec", now + 1000)).toBe(false);
  expect(await verifySignature(`ts=${now}`, body, "whsec", now)).toBe(false);
});

test("license rules", () => {
  const ends_at = "2026-12-01T00:00:00Z";
  expect(periodEnd({ status: "active", current_billing_period: { ends_at } })).toBe(Date.parse(ends_at) / 1000);
  expect(periodEnd({ status: "past_due", current_billing_period: { ends_at } })).toBe(Date.parse(ends_at) / 1000);
  expect(periodEnd({ status: "canceled", current_billing_period: null })).toBe(0);
  expect(periodEnd({ status: "paused", current_billing_period: { ends_at } })).toBe(0);
  expect(newKey()).toMatch(/^DM(-[A-HJ-NP-Z2-9]{4}){3}$/);
  expect(mayActivate(["a", "b"], "c")).toBe(true);
  expect(mayActivate(["a", "b", "c"], "d")).toBe(false);
  expect(mayActivate(["a", "b", "c"], "b")).toBe(true);
});

test("buyer pages: language, unconfigured checkout, escaping", async () => {
  const req = (url: string, headers: Record<string, string> = {}) => new Request(url, { headers });
  expect(pickLang(req("https://deskmonk.app/buy?lang=es", { Cookie: "lang=tr" }))).toBe("es");
  expect(pickLang(req("https://deskmonk.app/buy", { Cookie: "a=1; lang=tr" }))).toBe("tr");
  expect(pickLang(req("https://deskmonk.app/buy", { "Accept-Language": "sw-KE,tr;q=0.8" }))).toBe("tr");
  expect(pickLang(req("https://deskmonk.app/buy?lang=xx"))).toBe("en");

  const opts = { configured: false, plan: "yearly" as const, install: "", token: "", sandbox: true, prices: { monthly: "", yearly: "" }, country: null };
  const soon = buyPage(req("https://deskmonk.app/buy"), "tr", opts);
  expect(soon.status).toBe(503);
  expect(await soon.text()).toContain("Ödeme yakında açılıyor");

  const live = await buyPage(req("https://deskmonk.app/buy"), "en", { ...opts, configured: true, token: "</script><b>", prices: { monthly: "pri_m", yearly: "pri_y" } }).text();
  expect(live).not.toContain("</script><b>");
  expect(live).toContain('value="yearly" checked');
  expect(js("</script>")).toBe('"\\u003c/script>"');
});

test("Google Tag Manager waits for Allow where it needs consent, and when the country is unknown", async () => {
  const ads = (country: string | null) =>
    adsScript(Object.assign(new Request("https://deskmonk.app/ads.js"), country ? { cf: { country } } : {})).text();
  const us = await ads("US");
  expect(us).toContain("gtm.js?id=GTM-KK2TWW72");
  expect(us).not.toContain("consent");
  for (const c of ["DE", "NO", "GB", "CH", "TR", null]) {
    const js = await ads(c);
    expect(js).toContain('gtag("consent", "default"');
    // The tag is only inside load(), which runs after Allow or a kept "ads=1".
    expect(js.indexOf("gtm.js?id=")).toBeGreaterThan(js.indexOf("function load()"));
    expect(js.indexOf("gtm.js?id=")).toBeLessThan(js.indexOf("function keep("));
    expect(() => new Function(js)).not.toThrow();
  }
  expect(await ads("TR")).toContain('"İzin ver"');
  expect(await ads("DE")).toContain('"Ablehnen"');
});

test("language: chosen, then country, then browser; prices filled in", () => {
  const at = (url: string, country: string | null, headers: Record<string, string> = {}) =>
    Object.assign(new Request(url, { headers }), country ? { cf: { country } } : {});
  expect(pickLang(at("https://deskmonk.app/", "DE", { "Accept-Language": "en-US" }))).toBe("de");
  expect(pickLang(at("https://deskmonk.app/", "BR"))).toBe("pt");
  expect(pickLang(at("https://deskmonk.app/", "HK"))).toBe("zh-TW");
  expect(pickLang(at("https://deskmonk.app/", "DE", { Cookie: "lang=ja" }))).toBe("ja");
  expect(pickLang(at("https://deskmonk.app/?lang=ar", "DE", { Cookie: "lang=ja" }))).toBe("ar");
  expect(pickLang(at("https://deskmonk.app/", "US", { "Accept-Language": "nn-NO,en;q=0.5" }))).toBe("nb");
  expect(pickLang(at("https://deskmonk.app/", "XX", { "Accept-Language": "zh-Hant-TW" }))).toBe("zh-TW");
  expect(pickLang(at("https://deskmonk.app/", null))).toBe("en");


  expect(word("en", "meta.pdesc")).toContain("$2.50 a month or $25 a year");
  expect(money("de", USD.monthly)).toMatch(/2,50\s\$/);
  // Every language has every key, so no page shows English by accident.
  for (const lang of LANGS) expect(word(lang, "hero.title"), lang).not.toBe("");
});

test("world map puts a dot on each known country, none for unknown", () => {
  const svg = worldMap([{ country: "TR", today: 3 }, { country: "?", today: 1 }]);
  expect(svg.match(/class="spot"/g)).toHaveLength(1);
  expect(svg).toContain("Türkiye: 3");
  expect(svg).toContain('class="land" d="M');
});

test("webhook sender check: IPv4 inside Paddle's CIDR blocks only", () => {
  expect(inCidr("34.237.3.244", "34.237.3.244/32")).toBe(true);
  expect(inCidr("34.237.3.245", "34.237.3.244/32")).toBe(false);
  expect(inCidr("10.1.2.3", "10.0.0.0/8")).toBe(true);
  expect(inCidr("11.0.0.1", "10.0.0.0/8")).toBe(false);
  expect(inCidr("", "34.237.3.244/32")).toBe(false);
  expect(inCidr("2001:db8::1", "34.237.3.244/32")).toBe(false);
  expect(inCidr("34.237.3.244", "34.237.3.244/33")).toBe(false);
});

// ---------- mail ----------

/** schema.sql in Node's SQLite, behind the few D1 calls mail.ts uses. */
function memoryDb(): any {
  const { DatabaseSync } = require("node:sqlite");
  const db = new DatabaseSync(":memory:");
  db.exec(require("node:fs").readFileSync(`${__dirname}/../schema.sql`, "utf8"));
  const prepare = (sql: string) => {
    const run = (args: unknown[]) => ({
      first: async () => db.prepare(sql).get(...args) ?? null,
      all: async () => ({ results: db.prepare(sql).all(...args) }),
      run: async () => db.prepare(sql).run(...args),
      raw: async () => db.prepare(sql).all(...args).map((r: any) => Object.values(r)),
    });
    return { bind: (...args: unknown[]) => run(args), ...run([]) };
  };
  return { prepare, exec: (sql: string) => db.exec(sql), batch: async (stmts: any[]) => Promise.all(stmts.map((st) => st.all())) };
}

test("the key mail carries the key and how to use it", () => {
  const m = keyMail("DM-ABCD-EFGH-JKLM", "yearly");
  expect(m.text).toContain("DM-ABCD-EFGH-JKLM");
  expect(m.text).toContain("Have a license key?");
  expect(m.text).toContain("https://deskmonk.app/account");
});

test("contact form: checks input, keeps the message, limits how many", async () => {
  const env = { DB: memoryDb(), OWNER_EMAIL: "owner@example.com" }; // no RESEND_API_KEY: nothing is sent
  const post = (body: unknown) =>
    handleMail(new Request("https://deskmonk.app/api/contact", { method: "POST", body: JSON.stringify(body) }), env, "/api/contact");
  expect((await handleMail(new Request("https://deskmonk.app/x"), env, "/x"))).toBeNull();
  expect((await post({ message: "", install_id: "a" }))!.status).toBe(400);
  expect((await post({ message: "hi" }))!.status).toBe(400);
  expect((await post({ message: "hi", install_id: "a", email: "not an email" }))!.status).toBe(400);
  for (let i = 0; i < 3; i++) expect((await post({ message: `hi ${i}`, install_id: "a", email: "me@example.com", version: "0.5.0", os: "macos" }))!.status).toBe(200);
  expect((await post({ message: "4th", install_id: "a" }))!.status).toBe(429);
  expect((await post({ message: "other user", install_id: "b" }))!.status).toBe(200);
  const rows = (await env.DB.prepare("SELECT install_id, email, message FROM messages ORDER BY id").all()).results;
  expect(rows.map((r: any) => r.message)).toEqual(["hi 0", "hi 1", "hi 2", "other user"]);
  expect(rows[0].email).toBe("me@example.com");
});

test("weekly numbers leave out the operator's own installs", async () => {
  const db = memoryDb();
  const now = 2_000_000_000, day = 86400;
  const ev = (id: string, ts: number, kind = "open") =>
    db.exec(`INSERT INTO events(ts, install_id, version, os, client, kind, files, bytes, duplicate_bytes, junk_bytes, old_download_bytes, large_bytes)
             VALUES (${ts}, '${id}', '0.5.0', '${id === "win" ? "windows" : "macos"}', 'gui', '${kind}', 0, 0, 0, 0, 0, 0)`);
  ev("old", now - 30 * day); // installed long ago, still used
  ev("old", now - day);
  ev("new", now - 2 * day);
  ev("new", now - 2 * day, "scan");
  ev("win", now - 3 * day);
  ev("gone", now - 20 * day); // not used this week
  ev("mine", now - day);
  db.exec(`INSERT INTO ignored_installs VALUES ('mine', 0)`);
  db.exec(`INSERT INTO subscriptions(id, customer_id, status, period_end, license_key, event_at, plan, created_at) VALUES
    ('s1', 'c1', 'active', ${now + 20 * day}, 'K1', '', 'monthly', ${now - day}),
    ('s2', 'c2', 'active', ${now + 300 * day}, 'K2', '', 'yearly', ${now - 40 * day}),
    ('s3', 'c3', 'canceled', ${now - 2 * day}, 'K3', '', 'monthly', ${now - 60 * day}),
    ('s4', 'manual', 'active', ${now + 9 * day}, 'K4', '', 'yearly', ${now - day})`);
  const n = await weekNumbers(db, now);
  expect(n).toMatchObject({ newInstalls: 2, activeInstalls: 3, opens: 3, newSubs: 1, monthly: 1, yearly: 1, canceled: 1, messages: 0 });
  expect(n.os).toEqual([{ os: "macos", n: 2 }, { os: "windows", n: 1 }]);
  expect(weeklyText(n)).toContain("New: 2");
});

test("Pro now counts only paying customers, not your own computers or manual grants", async () => {
  (crypto.subtle as any).timingSafeEqual ??= (a: Uint8Array, b: Uint8Array) => a.every((x, i) => x === b[i]); // Workers-only API
  const env = { DB: memoryDb(), ADMIN_TOKEN: "test" } as any;
  const far = 2_000_000_000;
  env.DB.exec(`INSERT INTO ignored_installs VALUES ('my-mac', 0)`);
  env.DB.exec(`INSERT INTO subscriptions(id, customer_id, email, status, period_end, license_key, event_at, plan) VALUES
    ('real', 'c1', 'buyer@x.com', 'active', ${far}, 'K1', '', 'monthly'),
    ('grant', 'manual', 'me@x.com', 'active', ${far}, 'K2', '', 'yearly'),
    ('test-buy', 'c2', 'me@x.com', 'active', ${far}, 'K3', '', 'monthly')`);
  env.DB.exec(`INSERT INTO activations VALUES ('K2', 'my-mac', 0), ('K3', 'unseen-copy', 0)`);
  const res = await handleAdmin(new Request("https://deskmonk.app/admin", { headers: { Authorization: `Basic ${btoa("admin:test")}` } }), env, "/admin");
  const page = await res!.text();
  expect(page).toMatch(/Pro now<small>[^<]*<\/small><\/p><\/div>\s*<div class="v">1<\/div>/);
});

test("removing a computer frees its place, also on the computer it was bought on", async () => {
  const env = { DB: memoryDb() };
  env.DB.exec(`INSERT INTO subscriptions(id, customer_id, status, period_end, license_key, install_id, event_at, plan)
               VALUES ('s1', 'c1', 'active', 2000000000, 'DM-ABCD-EFGH-JKLM', 'buyer', '', 'monthly')`);
  env.DB.exec(`INSERT INTO activations VALUES ('DM-ABCD-EFGH-JKLM', 'buyer', 0), ('DM-ABCD-EFGH-JKLM', 'laptop', 0)`);
  const post = (path: string, body: unknown) =>
    handleLicense(new Request(`https://deskmonk.app${path}`, { method: "POST", body: JSON.stringify(body) }), env as any, path);
  expect((await post("/api/license/deactivate", { key: "dm-abcd-efgh-jklm", install_id: "buyer" }))!.status).toBe(200);
  const left = (await env.DB.prepare("SELECT install_id FROM activations").all()).results.map((r: any) => r.install_id);
  expect(left).toEqual(["laptop"]);
  // The buying computer's next start (no key, only its id) must not take the place back.
  expect(await (await post("/api/license/validate", { key: "", install_id: "buyer" }))!.json()).toEqual({ key: "", pro_until: 0 });
  expect((await post("/api/license/deactivate", { key: "", install_id: "buyer" }))!.status).toBe(404);
});

test("pelamx day pass: signed callback issues one day key, old apps read it as free", async () => {
  const env = { DB: memoryDb(), PARTNER_DM_SECRET: "s3cret" } as any;
  const order = "0123456789abcdef0123456789abcdef";
  const body = JSON.stringify({ order, user: "a@b.co", app_id: "deskmonk", plan: "day_pass", charge_ref: "daypass:deskmonk:a@b.co" });
  const sig = async (b: string) => {
    const k = await crypto.subtle.importKey("raw", new TextEncoder().encode("s3cret"), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    return [...new Uint8Array(await crypto.subtle.sign("HMAC", k, new TextEncoder().encode(b)))].map((x) => x.toString(16).padStart(2, "0")).join("");
  };
  const call = async (b: string, s: string) =>
    handleLicense(new Request("https://deskmonk.app/api/plx-balance/callback", { method: "POST", body: b, headers: { "x-plx-signature": s } }), env, "/api/plx-balance/callback");
  expect(await plxSignatureOk(body, await sig(body), "s3cret")).toBe(true);
  expect((await call(body, "00" + (await sig(body)).slice(2)))!.status).toBe(401);
  expect((await call(body.replace("day_pass", "pro_yearly"), await sig(body.replace("day_pass", "pro_yearly"))))!.status).toBe(400);
  const first: any = await (await call(body, await sig(body)))!.json();
  expect(first.licence_key).toMatch(/^DM(-[A-HJ-NP-Z2-9]{4}){3}$/);
  expect(first.expires_at - Math.floor(Date.now() / 1000)).toBeGreaterThan(86000);
  // A retry of the same order returns the same key, never a second one.
  expect(((await (await call(body, await sig(body)))!.json()) as any).licence_key).toBe(first.licence_key);
  const v: any = await (await handleLicense(new Request("https://deskmonk.app/api/license/validate", { method: "POST", body: JSON.stringify({ key: first.licence_key, install_id: "pc1" }) }), env, "/api/license/validate"))!.json();
  expect(v).toEqual({ key: first.licence_key, pro_until: 0, day_until: first.expires_at });
  // The claim link goes to pelamx with a fresh order.
  const r = (await handleLicense(new Request("https://deskmonk.app/day-pass"), env, "/day-pass"))!;
  expect(r.headers.get("location")).toMatch(/^https:\/\/pelamx\.com\/claim\?app=deskmonk&order=[0-9a-f]{32}&return=/);
});

test("refund: the last completed payment in full, then cancel at once", async () => {
  const calls: { method: string; url: string; body: any }[] = [];
  const real = globalThis.fetch;
  globalThis.fetch = (async (url: string, init: RequestInit = {}) => {
    calls.push({ method: init.method ?? "GET", url, body: init.body ? JSON.parse(String(init.body)) : undefined });
    if (url.includes("/transactions?")) {
      return Response.json({ data: [{ id: "txn_2", currency_code: "USD", details: { totals: { grand_total: "250" } } }] });
    }
    if (url.endsWith("/adjustments")) return Response.json({ data: { status: "pending_approval" } }, { status: 201 });
    return Response.json({ data: {} });
  }) as typeof fetch;
  try {
    const env = { DB: memoryDb(), PADDLE_ENV: "production", PADDLE_API_KEY: "test" } as any;
    const r = await refundLast(env, "sub_1", "asked by email");
    expect(r).toEqual({ ok: "Refund of txn_2 (2.50 USD) sent to Paddle (status: pending_approval). Subscription canceled; no more charges." });
    expect(calls.map((c) => `${c.method} ${c.url}`)).toEqual([
      "GET https://api.paddle.com/transactions?subscription_id=sub_1&status=completed&order_by=billed_at[DESC]&per_page=1",
      "POST https://api.paddle.com/adjustments",
      "POST https://api.paddle.com/subscriptions/sub_1/cancel",
    ]);
    expect(calls[1].body).toEqual({ action: "refund", type: "full", transaction_id: "txn_2", reason: "asked by email" });
    expect(calls[2].body).toEqual({ effective_from: "immediately" });
    expect(await refundLast({ ...env, PADDLE_API_KEY: undefined }, "sub_1", "x")).toEqual({ error: "PADDLE_API_KEY is not set." });
  } finally {
    globalThis.fetch = real;
  }
});

test("buy and account pages answer HEAD like GET, for uptime monitors", async () => {
  const env = { DB: {}, PADDLE_CLIENT_TOKEN: "t", PADDLE_PRICE_MONTHLY: "m", PADDLE_PRICE_YEARLY: "y" } as any;
  for (const path of ["/buy", "/account"]) {
    const res = await handleLicense(new Request(`https://deskmonk.app${path}`, { method: "HEAD" }), env, path);
    expect(res?.status).toBe(200);
  }
});

test("the key mail points at the field in the app, not at the checkout", () => {
  const { text } = keyMail("DM-AAAA-BBBB-CCCC", "monthly");
  expect(text).toContain("DM-AAAA-BBBB-CCCC");
  expect(text).toContain("Have a license key?");
  expect(text).not.toContain("Get Pro");
});

test("each platform's download comes from its own release file", () => {
  expect(platformOf("/downloads/deskMonk-0.5.3-macos.dmg")).toBe("macos");
  expect(platformOf("/downloads/deskmonk-windows-x86_64.zip")).toBe("windows-x86_64");
  expect(platformOf("/downloads/deskMonk-0.5.4-linux-x86_64.AppImage")).toBe("linux-x86_64");
  expect(platformOf("/downloads/update/deskmonk-0.5.3-linux-aarch64.tar.gz")).toBeNull();
  const mac = { version: "0.5.7", download: "/downloads/deskMonk-0.5.7-macos.dmg", size: 17628988 };
  expect(downloadLabel(mac, "macos")).toBe("v0.5.7 · DMG · 17 MB");
  expect(downloadLabel({ version: "0.5.6", download: "/downloads/x.AppImage", size: 11258360 }, "linux-x86_64")).toBe("v0.5.6 · AppImage · x86_64 · 11 MB");
  // withDownloads uses HTMLRewriter, which only the Workers runtime has: checked on the live page after a deploy.
});

test("where a visitor came from", () => {
  const home = new URL("https://deskmonk.app/");
  expect(sourceOf(home, null)).toBe("direct");
  expect(sourceOf(home, "https://deskmonk.app/pricing")).toBe("direct");
  expect(sourceOf(home, "https://www.google.com.tr/")).toBe("google");
  expect(sourceOf(home, "https://www.reddit.com/r/macapps/")).toBe("reddit.com");
  expect(sourceOf(home, "not a url")).toBe("direct");
  expect(sourceOf(new URL("https://deskmonk.app/?gclid=abc"), "https://www.google.com/")).toBe("google ads");
  expect(sourceOf(new URL("https://deskmonk.app/?utm_source=Newsletter<b>"), null)).toBe("newsletterb");
  expect(sourceOf(new URL("https://deskmonk.app/download/macos?from=reddit.com"), "https://deskmonk.app/")).toBe("reddit.com");
  expect(sourceOf(new URL(`https://deskmonk.app/?utm_source=${"x".repeat(100)}`), null)).toHaveLength(60);
});

test("one computer counts once: copies that send its machine id share it, with their earlier history and mine mark", async () => {
  const db = memoryDb();
  const m = "0123456789abcdef0123456789abcdef";
  const ev = (install_id: string, machine?: string, specs?: unknown[]) =>
    insert(db, parseEvent(JSON.stringify({ install_id, machine, version: "0.5.10", os: "macos", client: "gui", kind: "open", specs })), "TR");
  await ev("old-copy"); // before the update: no machine id
  db.exec(`INSERT INTO ignored_installs VALUES ('old-copy', 0)`);
  await ev("old-copy", undefined, [{ kind: "Bilgisayar", brand: "Apple", name: "MacBook Pro", detail: "" }]);
  await ev("old-copy", m); // updated
  await ev("second-copy", m); // terminal version, another user, a reinstall
  const ids = (await db.prepare("SELECT DISTINCT install_id AS id FROM events").all()).results.map((r: any) => r.id);
  expect(ids).toEqual([m]);
  expect((await db.prepare("SELECT install_id FROM ignored_installs WHERE install_id = ?1").bind(m).first())?.install_id).toBe(m);
  expect((await db.prepare("SELECT install_id FROM hardware").all()).results.map((r: any) => r.install_id)).toEqual([m]);
  await ev("phone"); // apps that send no machine id keep their own
  expect((await db.prepare("SELECT COUNT(DISTINCT install_id) AS n FROM events").first()).n).toBe(2);
  expect(() => parseEvent(`{"install_id":"a","machine":"NOT-HEX","version":"1","os":"macos","client":"gui","kind":"open"}`)).toThrow("invalid machine");
});
