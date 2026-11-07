//! deskMonk Pro: Paddle checkout, Paddle webhooks and license checks for the apps. The pages themselves are in pages.ts.
//! GET  /buy?plan=monthly|yearly[&install=<id>] checkout page (Paddle overlay); the app passes its install id, the website none
//! GET  /welcome?ref=<ref>                      after paying: polls /api/order/<ref>, then shows the license key
//! GET  /api/order/<ref>                        → {status: "pending"} or {status: "paid", key, plan}
//! GET|POST /account, POST /account/portal      a buyer looks up a key; Paddle's portal to cancel or change the card
//! POST /api/paddle                             Paddle webhook (subscription.*), signature checked; other products' prices are ignored
//! POST /api/license/validate {key, install_id} → {key, pro_until}; a key works on up to MAX_DEVICES installations
//! POST /api/license/portal {key}               → {url} of Paddle's page to cancel or change the subscription
//! POST /api/license/deactivate {key, install_id} → {ok}; frees one of the key's computers ("Remove from this computer")
//! GET  /day-pass, POST /checkout/day-pass      a free day claimed on pelamx.com with PLX: redirect / {ref, url} to pelamx's claim page
//! POST /api/plx-balance/callback               pelamx's signed call: issues the day key (App Lock + Vault, cleaning stays limited)

export interface LicenseEnv extends MailEnv {
  DB: D1Database;
  PADDLE_ENV?: string;
  PADDLE_CLIENT_TOKEN?: string;
  PADDLE_API_KEY?: string;
  PADDLE_WEBHOOK_SECRET?: string;
  PADDLE_PRICE_MONTHLY?: string;
  PADDLE_PRICE_YEARLY?: string;
  /** Shared HMAC secret with pelamx.com (its PARTNER_DM_SECRET); unset = no day pass. */
  PARTNER_DM_SECRET?: string;
}

import { keyMail, sendMail, type MailEnv } from "./mail";
import { accountPage, buyPage, pickLang, t, welcomePage } from "./pages";

export const MAX_DEVICES = 3;
/** Webhooks older than this are refused (replays). */
const SIGNATURE_TOLERANCE = 300;
const INSTALL_ID = /^[A-Za-z0-9._-]{1,64}$/;
const KEY = /^DM(-[A-HJ-NP-Z2-9]{4}){3}$/;
const KEY_CHARS = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
/** Order reference made by the /buy page: 16 random bytes in hex. */
const REF = /^[0-9a-f]{32}$/;
/** How long after the purchase /api/order still reveals the key for a ref (a reference can sit in browser history). */
const ORDER_REVEAL = 2 * 86400;

/** Returns a response for license routes, or null for any other path. */
export async function handleLicense(req: Request, env: LicenseEnv, path: string): Promise<Response | null> {
  // HEAD as GET: uptime monitors and link checkers ask that way; the runtime leaves out the body.
  const read = req.method === "GET" || req.method === "HEAD";
  if (read && path === "/buy") return buy(req, env);
  if (read && path === "/welcome") {
    const ref = new URL(req.url).searchParams.get("ref") ?? "";
    return REF.test(ref) ? welcomePage(req, pickLang(req), ref) : Response.redirect(new URL("/buy", req.url).toString(), 302);
  }
  if (read && path === "/day-pass") return env.PARTNER_DM_SECRET ? Response.redirect(dayPassClaimUrl(), 302) : Response.redirect(new URL("/buy", req.url).toString(), 302);
  if (req.method === "POST" && path === "/checkout/day-pass") {
    if (!env.PARTNER_DM_SECRET) return json({ error: "not_configured" }, 503);
    const url = dayPassClaimUrl();
    return json({ ref: new URL(url).searchParams.get("order"), url });
  }
  if (req.method === "POST" && path === "/api/plx-balance/callback") return dayPassCallback(req, env);
  if (req.method === "GET" && path.startsWith("/api/order/")) return order(env.DB, path.slice("/api/order/".length));
  if (path === "/account" && (read || req.method === "POST")) return account(req, env);
  if (req.method === "POST" && path === "/account/portal") return accountPortal(req, env);
  if (req.method === "POST" && path === "/api/paddle") return webhook(req, env);
  if (req.method === "POST" && (path === "/api/license/validate" || path === "/api/license/portal" || path === "/api/license/deactivate")) {
    const body = await req.text();
    if (body.length > 1024) return json({ error: "bad_request" }, 400);
    let v: any;
    try {
      v = JSON.parse(body);
    } catch {
      return json({ error: "bad_request" }, 400);
    }
    const key = typeof v?.key === "string" ? v.key.trim().toUpperCase() : "";
    if (key !== "" && !KEY.test(key)) return json({ error: "unknown_key" }, 404);
    if (path === "/api/license/portal") {
      const url = key === "" ? null : await portalUrl(env, key);
      return url !== null ? json({ url }) : json({ error: key === "" ? "unknown_key" : "paddle" }, key === "" ? 404 : 502);
    }
    const install = typeof v?.install_id === "string" && INSTALL_ID.test(v.install_id) ? v.install_id : null;
    if (install === null) return json({ error: "bad_request" }, 400);
    if (path === "/api/license/deactivate") return key === "" ? json({ error: "unknown_key" }, 404) : deactivate(env.DB, key, install);
    return validate(env.DB, key, install);
  }
  return null;
}

/** Frees a computer's place on a key. If it is the computer the key was bought on, it also stops being found by its
 *  installation id alone, or the next start there would take the place again. */
async function deactivate(db: D1Database, key: string, install: string): Promise<Response> {
  await db.batch([
    db.prepare("DELETE FROM activations WHERE license_key = ?1 AND install_id = ?2").bind(key, install),
    db.prepare("UPDATE subscriptions SET install_id = NULL WHERE license_key = ?1 AND install_id = ?2").bind(key, install),
  ]);
  return json({ ok: true });
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

// ---------- pure parts (tested) ----------

/** Checks a `Paddle-Signature: ts=…;h1=…` header: HMAC-SHA256 of "ts:body" with the webhook secret. */
export async function verifySignature(header: string, body: string, secret: string, now: number): Promise<boolean> {
  const parts = header.split(";").map((p) => p.split("="));
  const ts = parts.find(([k]) => k === "ts")?.[1] ?? "";
  const sigs = parts.filter(([k]) => k === "h1").map(([, v]) => v);
  if (!/^\d+$/.test(ts) || Math.abs(now - Number(ts)) > SIGNATURE_TOLERANCE || sigs.length === 0) return false;
  const enc = new TextEncoder();
  const hmacKey = await crypto.subtle.importKey("raw", enc.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", hmacKey, enc.encode(`${ts}:${body}`)));
  const hex = [...mac].map((b) => b.toString(16).padStart(2, "0")).join("");
  // Constant time per candidate; there are at most two during a secret rotation.
  return sigs.some((s) => s.length === hex.length && [...s].reduce((acc, c, i) => acc | (c.charCodeAt(0) ^ hex.charCodeAt(i)), 0) === 0);
}

/** End of the paid period (unix seconds) of a Paddle subscription; 0 = not Pro. */
export function periodEnd(sub: any): number {
  if (!["active", "trialing", "past_due"].includes(sub?.status)) return 0;
  const end = Date.parse(sub?.current_billing_period?.ends_at ?? "");
  return Number.isFinite(end) ? Math.floor(end / 1000) : 0;
}

export function newKey(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(12));
  const chars = [...bytes].map((b) => KEY_CHARS[b % KEY_CHARS.length]).join("");
  return `DM-${chars.slice(0, 4)}-${chars.slice(4, 8)}-${chars.slice(8)}`;
}

/** Whether `install` may use a key already active on `devices`. */
export function mayActivate(devices: string[], install: string): boolean {
  return devices.includes(install) || devices.length < MAX_DEVICES;
}

// ---------- routes ----------

/** Whether an IPv4 address is inside a CIDR block such as "34.237.3.244/32". */
export function inCidr(ip: string, cidr: string): boolean {
  const n = (a: string) => {
    const p = a.split(".").map(Number);
    return p.length === 4 && p.every((x) => Number.isInteger(x) && x >= 0 && x <= 255) ? ((p[0] << 24) | (p[1] << 16) | (p[2] << 8) | p[3]) >>> 0 : null;
  };
  const [base, bits] = cidr.split("/");
  const a = n(ip), b = n(base), len = Number(bits ?? 32);
  if (a === null || b === null || !(len >= 0 && len <= 32)) return false;
  const mask = len === 0 ? 0 : (~0 << (32 - len)) >>> 0;
  return ((a & mask) >>> 0) === ((b & mask) >>> 0);
}

/** Paddle's webhook servers, from Paddle's own list (it can change), kept for an hour per isolate. */
let senders: { at: number; cidrs: string[] } | null = null;
async function fromPaddle(req: Request, env: LicenseEnv): Promise<boolean> {
  if (!senders || Date.now() - senders.at > 3600_000) {
    try {
      const res = await fetch(`${paddleApi(env)}/ips`);
      const cidrs = res.ok ? ((await res.json()) as { data?: { ipv4_cidrs?: unknown } }).data?.ipv4_cidrs : null;
      if (Array.isArray(cidrs) && cidrs.every((c) => typeof c === "string")) senders = { at: Date.now(), cidrs };
    } catch {
      // Paddle's list is unreachable: an older copy still works; with none, the delivery is refused and Paddle retries.
    }
  }
  const ip = req.headers.get("CF-Connecting-IP") ?? "";
  return !!senders && senders.cidrs.some((c) => inCidr(ip, c));
}

async function webhook(req: Request, env: LicenseEnv): Promise<Response> {
  if (!(await fromPaddle(req, env))) return new Response("not from Paddle", { status: 403 });
  const body = await req.text();
  const secret = env.PADDLE_WEBHOOK_SECRET ?? "";
  if (secret === "" || !(await verifySignature(req.headers.get("Paddle-Signature") ?? "", body, secret, Math.floor(Date.now() / 1000)))) {
    return new Response("bad signature", { status: 401 });
  }
  const ev = JSON.parse(body);
  if (typeof ev?.event_type !== "string" || !ev.event_type.startsWith("subscription.")) return new Response(null, { status: 200 });
  const sub = ev.data;
  if (typeof sub?.id !== "string" || typeof sub?.customer_id !== "string") return new Response("bad event", { status: 400 });
  // The Paddle account may sell other products too (its notifications cover all of them): only deskMonk's prices count.
  const plan = planOf(sub, env);
  if (plan === null) return new Response(null, { status: 200 });
  const install = typeof sub.custom_data?.install_id === "string" && INSTALL_ID.test(sub.custom_data.install_id) ? sub.custom_data.install_id : null;
  const txn = typeof sub.transaction_id === "string" ? sub.transaction_id : null;
  const ref = typeof sub.custom_data?.ref === "string" && REF.test(sub.custom_data.ref) ? sub.custom_data.ref : null;
  const key = newKey();
  // Events can arrive out of order: an older one never overwrites a newer state. The key is made once.
  const created = Date.parse(sub.created_at ?? "");
  await env.DB.prepare(
    `INSERT INTO subscriptions(id, customer_id, status, period_end, license_key, install_id, txn, event_at, plan, created_at, ref)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
     ON CONFLICT(id) DO UPDATE SET customer_id = excluded.customer_id, status = excluded.status, period_end = excluded.period_end,
       install_id = COALESCE(subscriptions.install_id, excluded.install_id), txn = COALESCE(subscriptions.txn, excluded.txn),
       event_at = excluded.event_at, plan = excluded.plan, created_at = COALESCE(subscriptions.created_at, excluded.created_at),
       ref = COALESCE(subscriptions.ref, excluded.ref)
     WHERE excluded.event_at >= subscriptions.event_at`,
  )
    .bind(sub.id, sub.customer_id, String(sub.status), periodEnd(sub), key, install, txn, String(ev.occurred_at ?? ""), plan,
      Number.isFinite(created) ? Math.floor(created / 1000) : Math.floor(Date.now() / 1000), ref)
    .run();
  // The buyer's email, asked from Paddle once (subscription events do not carry it): for the admin panel, and the
  // moment it is first known the buyer is mailed their key, so that mail goes out once per subscription.
  const known = await env.DB.prepare("SELECT email, license_key, plan FROM subscriptions WHERE id = ?1")
    .bind(sub.id)
    .first<{ email: string | null; license_key: string; plan: string }>();
  if (known !== null && known.email === null) {
    const email = await customerEmail(env, sub.customer_id);
    if (email !== null) {
      await env.DB.prepare("UPDATE subscriptions SET email = ?1 WHERE id = ?2").bind(email, sub.id).run();
      await sendMail(env, { to: email, ...keyMail(known.license_key, known.plan), replyTo: env.OWNER_EMAIL });
    }
  }
  if (install !== null) {
    await env.DB.prepare(
      `INSERT OR IGNORE INTO activations(license_key, install_id, ts) SELECT license_key, ?2, ?3 FROM subscriptions WHERE id = ?1`,
    )
      .bind(sub.id, install, Math.floor(Date.now() / 1000))
      .run();
  }
  return new Response(null, { status: 200 });
}

async function validate(db: D1Database, key: string, install: string): Promise<Response> {
  type Sub = { license_key: string; period_end: number; plan: string | null };
  const sub =
    key !== ""
      ? await db.prepare("SELECT license_key, period_end, plan FROM subscriptions WHERE license_key = ?1").bind(key).first<Sub>()
      : // No key yet: a purchase made from this installation (the app checks right after the checkout).
        await db
          .prepare("SELECT license_key, period_end, plan FROM subscriptions WHERE install_id = ?1 ORDER BY period_end DESC LIMIT 1")
          .bind(install)
          .first<Sub>();
  if (sub === null) return key !== "" ? json({ error: "unknown_key" }, 404) : json({ key: "", pro_until: 0 });
  const devices = (await db.prepare("SELECT install_id FROM activations WHERE license_key = ?1").bind(sub.license_key).raw<[string]>()).map(([id]) => id);
  if (!mayActivate(devices, install)) return json({ error: "too_many_devices" }, 403);
  if (!devices.includes(install)) {
    await db
      .prepare("INSERT OR IGNORE INTO activations(license_key, install_id, ts) VALUES (?1, ?2, ?3)")
      .bind(sub.license_key, install, Math.floor(Date.now() / 1000))
      .run();
  }
  // A day key is not Pro: apps before the day pass read pro_until only, so they treat it as the free plan.
  if (sub.plan === "day") return json({ key: sub.license_key, pro_until: 0, day_until: sub.period_end });
  return json({ key: sub.license_key, pro_until: sub.period_end });
}

/** pelamx.com's claim page for a new day-pass order. */
function dayPassClaimUrl(): string {
  const ref = [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, "0")).join("");
  return `https://pelamx.com/claim?app=deskmonk&order=${ref}&return=${encodeURIComponent("https://deskmonk.app/")}`;
}

/** pelamx's signed callback after a free day was claimed there: issues a 24-hour day key, once per order. pelamx keeps
 *  one key per account, so this is reached once per account. */
async function dayPassCallback(req: Request, env: LicenseEnv): Promise<Response> {
  const raw = await req.text();
  const secret = env.PARTNER_DM_SECRET ?? "";
  if (secret === "" || raw.length > 2048 || !(await plxSignatureOk(raw, req.headers.get("x-plx-signature") ?? "", secret))) {
    return json({ error: "bad_signature" }, 401);
  }
  let p: any;
  try {
    p = JSON.parse(raw);
  } catch {
    return json({ error: "bad_request" }, 400);
  }
  if (p?.plan !== "day_pass" || typeof p.order !== "string" || !REF.test(p.order)) return json({ error: "bad_request" }, 400);
  const id = `pelamx_${p.order}`;
  const at = Math.floor(Date.now() / 1000);
  await env.DB.prepare(
    `INSERT OR IGNORE INTO subscriptions(id, customer_id, status, period_end, license_key, install_id, txn, event_at, plan, created_at, email, ref)
     VALUES (?1, 'pelamx', 'active', ?2, ?3, NULL, NULL, ?4, 'day', ?5, ?6, ?7)`,
  )
    .bind(id, at + 86400, newKey(), new Date(at * 1000).toISOString(), at, typeof p.user === "string" ? p.user.slice(0, 254) : null, p.order)
    .run();
  const row = await env.DB.prepare("SELECT license_key, period_end FROM subscriptions WHERE id = ?1").bind(id).first<{ license_key: string; period_end: number }>();
  return row === null ? json({ error: "not_issued" }, 500) : json({ licence_key: row.license_key, expires_at: row.period_end });
}

/** pelamx signs the raw body: hex HMAC-SHA256 with the shared secret, in `x-plx-signature`. */
export async function plxSignatureOk(body: string, given: string, secret: string): Promise<boolean> {
  const enc = new TextEncoder();
  const hmacKey = await crypto.subtle.importKey("raw", enc.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const hex = [...new Uint8Array(await crypto.subtle.sign("HMAC", hmacKey, enc.encode(body)))].map((b) => b.toString(16).padStart(2, "0")).join("");
  return given.length === hex.length && [...given].reduce((acc, c, i) => acc | (c.charCodeAt(0) ^ hex.charCodeAt(i)), 0) === 0;
}

/** "monthly"/"yearly" when the subscription is for one of deskMonk's prices, else null. */
export function planOf(sub: any, env: LicenseEnv): "monthly" | "yearly" | null {
  const ids = new Set((Array.isArray(sub?.items) ? sub.items : []).map((i: any) => i?.price?.id).filter(Boolean));
  if (env.PADDLE_PRICE_YEARLY && ids.has(env.PADDLE_PRICE_YEARLY)) return "yearly";
  if (env.PADDLE_PRICE_MONTHLY && ids.has(env.PADDLE_PRICE_MONTHLY)) return "monthly";
  return null;
}

async function customerEmail(env: LicenseEnv, customer: string): Promise<string | null> {
  if (!env.PADDLE_API_KEY) return null;
  try {
    const res = await fetch(`${paddleApi(env)}/customers/${encodeURIComponent(customer)}`, { headers: { Authorization: `Bearer ${env.PADDLE_API_KEY}` } });
    const email = res.ok ? ((await res.json()) as any)?.data?.email : null;
    return typeof email === "string" ? email : null;
  } catch {
    return null;
  }
}

function paddleApi(env: LicenseEnv): string {
  return env.PADDLE_ENV === "production" ? "https://api.paddle.com" : "https://sandbox-api.paddle.com";
}

/** For /admin: refunds the subscription's last paid transaction in full and cancels the subscription at once.
 *  Paddle reviews refunds (usually approves them within minutes) and pays the money back; the license ends here now. */
export async function refundLast(env: LicenseEnv, subId: string, reason: string): Promise<{ ok: string } | { error: string }> {
  if (!env.PADDLE_API_KEY) return { error: "PADDLE_API_KEY is not set." };
  const call = async (method: string, path: string, body?: unknown) => {
    const res = await fetch(`${paddleApi(env)}${path}`, {
      method,
      headers: { Authorization: `Bearer ${env.PADDLE_API_KEY}`, "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const v = (await res.json().catch(() => ({}))) as any;
    const why = `${res.status} ${v?.error?.detail ?? v?.error?.code ?? ""}`.trim();
    // 403 = the key lacks a permission; Paddle lets you tick more on an existing key.
    const fix = res.status === 403 ? ". In Paddle > Developer tools > Authentication, give the API key: Transactions read, Adjustments write, Subscriptions write" : "";
    return { ok: res.ok, v, why: why + fix };
  };
  try {
    const list = await call("GET", `/transactions?subscription_id=${encodeURIComponent(subId)}&status=completed&order_by=billed_at[DESC]&per_page=1`);
    const txn = list.ok ? list.v?.data?.[0] : undefined;
    if (typeof txn?.id !== "string") return { error: list.ok ? "Paddle has no completed payment for this subscription." : `Paddle: ${list.why}` };
    const refund = await call("POST", "/adjustments", { action: "refund", type: "full", transaction_id: txn.id, reason });
    if (!refund.ok) return { error: `Paddle refused the refund of ${txn.id}: ${refund.why}` };
    const cancel = await call("POST", `/subscriptions/${encodeURIComponent(subId)}/cancel`, { effective_from: "immediately" });
    const total = txn?.details?.totals?.grand_total, currency = txn?.currency_code;
    const amount = typeof total === "string" && typeof currency === "string" ? ` (${(Number(total) / 100).toFixed(2)} ${currency})` : "";
    return {
      ok: `Refund of ${txn.id}${amount} sent to Paddle (status: ${refund.v?.data?.status ?? "?"}).` +
        (cancel.ok ? " Subscription canceled; no more charges." : ` Cancel it in Paddle: ${cancel.why}.`),
    };
  } catch (e) {
    return { error: `Could not reach Paddle: ${e}` };
  }
}

/** Paddle's customer-portal page for the subscription of `key`, or null (unknown key, issued by hand, or Paddle failed). */
async function portalUrl(env: LicenseEnv, key: string): Promise<string | null> {
  const sub = await env.DB.prepare("SELECT id, customer_id FROM subscriptions WHERE license_key = ?1").bind(key).first<{ id: string; customer_id: string }>();
  if (sub === null || sub.customer_id === "manual" || sub.customer_id === "pelamx" || !env.PADDLE_API_KEY) return null;
  try {
    const res = await fetch(`${paddleApi(env)}/customers/${encodeURIComponent(sub.customer_id)}/portal-sessions`, {
      method: "POST",
      headers: { Authorization: `Bearer ${env.PADDLE_API_KEY}`, "Content-Type": "application/json" },
      body: JSON.stringify({ subscription_ids: [sub.id] }),
    });
    const url = res.ok ? ((await res.json()) as any)?.data?.urls?.general?.overview : undefined;
    return typeof url === "string" ? url : null;
  } catch {
    return null;
  }
}

function buy(req: Request, env: LicenseEnv): Response {
  const url = new URL(req.url);
  // From the app: its install id, so it turns Pro by itself. From the website: none; the buyer gets a key.
  const install = url.searchParams.get("install") ?? "";
  if (install !== "" && !INSTALL_ID.test(install)) return new Response("bad install id", { status: 400 });
  const country = (req as { cf?: { country?: unknown } }).cf?.country;
  return buyPage(req, pickLang(req), {
    configured: !!(env.PADDLE_CLIENT_TOKEN && env.PADDLE_PRICE_MONTHLY && env.PADDLE_PRICE_YEARLY),
    plan: url.searchParams.get("plan") === "yearly" ? "yearly" : "monthly",
    install,
    token: env.PADDLE_CLIENT_TOKEN ?? "",
    sandbox: env.PADDLE_ENV !== "production",
    prices: { monthly: env.PADDLE_PRICE_MONTHLY ?? "", yearly: env.PADDLE_PRICE_YEARLY ?? "" },
    country: typeof country === "string" && /^[A-Z]{2}$/.test(country) && country !== "XX" && country !== "T1" ? country : null,
  });
}

async function order(db: D1Database, ref: string): Promise<Response> {
  if (!REF.test(ref)) return json({ error: "bad_request" }, 400);
  const sub = await db
    .prepare("SELECT license_key, plan FROM subscriptions WHERE ref = ?1 AND created_at >= ?2")
    .bind(ref, Math.floor(Date.now() / 1000) - ORDER_REVEAL)
    .first<{ license_key: string; plan: string | null }>();
  // Paddle's webhook usually lands a few seconds after the overlay closes; until then the page keeps asking.
  const res = sub === null ? json({ status: "pending" }) : json({ status: "paid", key: sub.license_key, plan: sub.plan });
  res.headers.set("Cache-Control", "no-store");
  return res;
}

/** The key from a form post (application/x-www-form-urlencoded), normalised; "" when missing or malformed. */
async function formKey(req: Request): Promise<string> {
  try {
    const v = (await req.formData()).get("key");
    const key = typeof v === "string" ? v.trim().toUpperCase() : "";
    return KEY.test(key) ? key : "";
  } catch {
    return "";
  }
}

async function account(req: Request, env: LicenseEnv): Promise<Response> {
  const lang = pickLang(req);
  if (req.method !== "POST") return accountPage(req, lang, null);
  const key = await formKey(req);
  const sub =
    key === ""
      ? null
      : await env.DB.prepare(
          `SELECT license_key, plan, status, period_end, customer_id,
                  (SELECT COUNT(*) FROM activations a WHERE a.license_key = s.license_key) AS devices
           FROM subscriptions s WHERE license_key = ?1`,
        )
          .bind(key)
          .first<{ license_key: string; plan: string | null; status: string; period_end: number; customer_id: string; devices: number }>();
  if (sub === null) return accountPage(req, lang, null, t(lang).unknown);
  return accountPage(req, lang, {
    key: sub.license_key,
    plan: sub.plan,
    status: sub.status,
    periodEnd: sub.period_end,
    devices: sub.devices,
    maxDevices: MAX_DEVICES,
    paddle: sub.customer_id !== "manual",
  });
}

async function accountPortal(req: Request, env: LicenseEnv): Promise<Response> {
  const lang = pickLang(req);
  const key = await formKey(req);
  const url = key === "" ? null : await portalUrl(env, key);
  if (url !== null) return Response.redirect(url, 303);
  return accountPage(req, lang, null, t(lang).noPortal);
}
