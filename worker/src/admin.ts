//! The operator's panel at /admin (HTTP Basic auth: user "admin", password = ADMIN_TOKEN secret).
//! GET  /admin                 analytics + subscriptions; ?q= searches subscriptions
//! GET  /admin/user/<id>       one installation: usage, hardware, licenses, every event
//! GET  /admin/mobile-live     the Mobile card's live part, refetched by the page
//! GET  /admin/export.csv      ?what=installs|subscriptions
//! POST /admin/mine            mark an installation as the operator's own (left out of every count)
//! POST /admin/issue           a Pro license nobody paid for (press, support, testers)
//! POST /admin/{extend,devices,rotate,end,delete}   manage one subscription
//! POST /admin/refund          refund a Paddle subscription's last payment in full, cancel it, end Pro (money moves in Paddle)

import type { Env } from "./index";
import { USD } from "./i18n";
import { MAX_DEVICES, newKey, refundLast } from "./license";
import { COUNTRY_POINTS, LAND_ROWS, MAP_NORTH, MAP_STEP } from "./world";

const DAY = 86400;
/** The operator's day: Turkey time (UTC+3, no daylight saving). */
const TZ = 3 * 3600;
/** USD list prices (i18n.ts); Paddle's own figures are in its dashboard. */
const PRICE = USD;
const OSES = ["macos", "windows", "linux"];
const MOBILE = ["ios", "android"];
/** A phone counts as live while its heartbeat keeps arriving: every 4 minutes on screen (telemetry.dart in the app). */
const LIVE = 300;
/** Phone or tablet, as each app reports it from its screen size (hardware.dart, spec kind "Tür"). */
const MO_T = "t AS (SELECT h.install_id, json_extract(s.value, '$.name') AS type FROM hardware h, json_each(h.specs) s WHERE json_extract(s.value, '$.kind') = 'Tür')";
/** Phone events minus the operator's own installations, each with its type, and each installation's first and last event. */
const MO = `${MO_T}, mo AS (SELECT e.*, t.type FROM events e LEFT JOIN t ON t.install_id = e.install_id WHERE e.os IN ('ios', 'android') AND e.install_id NOT IN (SELECT install_id FROM ignored_installs))`;
const MO_F = "f AS (SELECT install_id, MAX(os) AS os, MAX(type) AS type, MIN(ts) AS first, MAX(ts) AS last FROM mo GROUP BY install_id)";
const STORE_NAME: Record<string, string> = { appstore: "App Store", playstore: "Google Play" };
/** Events minus the operator's own installations, each with its type (desktops have none). */
const EV = `${MO_T}, ev AS (SELECT e.*, t.type FROM events e LEFT JOIN t ON t.install_id = e.install_id WHERE e.install_id NOT IN (SELECT install_id FROM ignored_installs))`;
const KIND_LABEL: Record<string, string> = {
  Bilgisayar: "Computer", Sistem: "System", "İşlemci": "Processor", Bellek: "Memory", "Ekran kartı": "Graphics",
  Ekran: "Display", Depolama: "Storage", "Disk alanı": "Disk space", Pil: "Battery", Telefon: "Phone", "Tür": "Type",
};

type Row = Record<string, any>;
interface Notice { message?: string; error?: string }

// ---------- entry ----------

export async function handleAdmin(req: Request, env: Env, path: string): Promise<Response | null> {
  if (path !== "/admin" && !path.startsWith("/admin/")) return null;
  const token = env.ADMIN_TOKEN ?? "";
  if (token.length < 4) return new Response("ADMIN_TOKEN must be set (at least 4 characters)", { status: 500 });
  if (!(await sameSecret(req.headers.get("Authorization") ?? "", `Basic ${btoa(`admin:${token}`)}`))) {
    return new Response(null, { status: 401, headers: { "WWW-Authenticate": 'Basic realm="deskMonk admin"' } });
  }
  const url = new URL(req.url);
  if (req.method === "GET") {
    if (path === "/admin") return html(await dashboard(env, url.searchParams.get("q")?.trim() ?? "", {}));
    if (path.startsWith("/admin/user/")) return html(await userPage(env.DB, decodeURIComponent(path.slice("/admin/user/".length))));
    if (path === "/admin/export.csv") return exportCsv(env.DB, url.searchParams.get("what") ?? "");
    if (path === "/admin/mobile-live") return html(mobileLive(await mobileLiveData(env.DB, now()), now()));
    return new Response("Not found", { status: 404 });
  }
  if (req.method !== "POST") return new Response("Not found", { status: 404 });
  // A browser that knows the password sends it with any request, also a form posted from another site.
  if (!sameOrigin(req)) return new Response("Cross-site request refused", { status: 403 });
  const form = await req.formData();
  const action = path.slice("/admin/".length);
  if (action === "mine") return mine(env.DB, form);
  if (action === "issue") return issue(env, form);
  if (["extend", "devices", "rotate", "end", "delete", "refund"].includes(action)) return manage(env, form, action);
  return new Response("Not found", { status: 404 });
}

/** Compares hashes, so neither the length nor the first wrong character leaks through timing. */
async function sameSecret(a: string, b: string): Promise<boolean> {
  const hash = async (s: string) => new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)));
  return crypto.subtle.timingSafeEqual(await hash(a), await hash(b));
}

function sameOrigin(req: Request): boolean {
  const origin = req.headers.get("Origin");
  if (origin === null) return true;
  try {
    return new URL(origin).origin === new URL(req.url).origin;
  } catch {
    return false;
  }
}

function html(body: string): Response {
  return new Response(body, { headers: { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store" } });
}

const now = () => Math.floor(Date.now() / 1000);

/** Written after the change it describes; never fails the change. */
async function audit(db: D1Database, action: string, ref: string | null, detail: string) {
  try {
    await db.prepare("INSERT INTO admin_log(ts, action, ref, detail) VALUES (?1, ?2, ?3, ?4)").bind(now(), action, ref, detail).run();
  } catch (e) {
    console.error("admin log", e);
  }
}

// ---------- actions ----------

async function mine(db: D1Database, form: FormData): Promise<Response> {
  const id = String(form.get("install") ?? "").trim();
  if (/^[A-Za-z0-9._-]{1,64}$/.test(id)) {
    if (form.get("mine") === "1") await db.prepare("INSERT OR IGNORE INTO ignored_installs(install_id, ts) VALUES (?1, ?2)").bind(id, now()).run();
    else await db.prepare("DELETE FROM ignored_installs WHERE install_id = ?1").bind(id).run();
  }
  const back = String(form.get("back") ?? "");
  return new Response(null, { status: 303, headers: { Location: back.startsWith("/admin") ? back : "/admin#users" } });
}

async function issue(env: Env, form: FormData): Promise<Response> {
  const email = String(form.get("email") ?? "").trim();
  if (email.length > 200 || (email !== "" && !/^[^\s@]+@[^\s@]+$/.test(email))) return html(await dashboard(env, "", { error: "That email address does not look right." }));
  const plan = form.get("plan") === "monthly" ? "monthly" : "yearly";
  const rawDays = String(form.get("days") ?? "").trim();
  const days = rawDays === "" ? (plan === "yearly" ? 365 : 31) : Number(rawDays);
  if (!Number.isInteger(days) || days < 1 || days > 3650) return html(await dashboard(env, "", { error: "Days must be between 1 and 3650." }));
  const id = `manual_${[...crypto.getRandomValues(new Uint8Array(5))].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
  const key = newKey();
  const end = now() + days * DAY;
  await env.DB.prepare(
    `INSERT INTO subscriptions(id, customer_id, status, period_end, license_key, install_id, txn, event_at, plan, created_at, email)
     VALUES (?1, 'manual', 'active', ?2, ?3, NULL, NULL, ?4, ?5, ?6, ?7)`,
  )
    .bind(id, end, key, new Date().toISOString(), plan, now(), email || null)
    .run();
  await audit(env.DB, "issue", id, `${plan}, ${days} days, ends ${date(end)}${email ? `, ${email}` : ""}`);
  return html(await dashboard(env, id, { message: `Issued ${id}: ${days} days, ends ${date(end)}. Key: ${key}` }));
}

async function manage(env: Env, form: FormData, action: string): Promise<Response> {
  const db = env.DB;
  const id = String(form.get("id") ?? "").trim();
  const sub = await db.prepare("SELECT * FROM subscriptions WHERE id = ?1").bind(id).first<Row>();
  if (sub === null) return html(await dashboard(env, "", { error: "That subscription no longer exists." }));
  const paddle = sub.customer_id !== "manual";
  let message: string;
  switch (action) {
    case "extend": {
      const days = Number(form.get("days"));
      if (!Number.isInteger(days) || days < 1 || days > 3650) return html(await dashboard(env, id, { error: "Days must be between 1 and 3650." }));
      // From whichever is later, so a lapsed license gets the full days.
      const end = Math.max(sub.period_end, now()) + days * DAY;
      await db.prepare("UPDATE subscriptions SET period_end = ?1, status = CASE WHEN status = 'canceled' THEN 'active' ELSE status END WHERE id = ?2").bind(end, id).run();
      await audit(db, "extend", id, `+${days} days: ${date(sub.period_end)} → ${date(end)}`);
      message = `Extended by ${days} days, now ends ${date(end)}.${paddle ? " Paddle's next renewal event sets its own date again." : ""}`;
      break;
    }
    case "devices": {
      const { meta } = await db.prepare("DELETE FROM activations WHERE license_key = ?1").bind(sub.license_key).run();
      await audit(db, "free devices", id, `freed ${meta.changes ?? 0}`);
      message = `Freed ${meta.changes ?? 0} device slots. The key can be used on ${MAX_DEVICES} computers again.`;
      break;
    }
    case "rotate": {
      const key = newKey();
      await db.batch([
        db.prepare("DELETE FROM activations WHERE license_key = ?1").bind(sub.license_key),
        db.prepare("UPDATE subscriptions SET license_key = ?1 WHERE id = ?2").bind(key, id),
      ]);
      await audit(db, "new key", id, "old key disabled");
      message = `New key ${key}. The old key no longer works; each computer has to enter the new one.`;
      break;
    }
    case "refund": {
      if (!paddle) return html(await dashboard(env, id, { error: "Issued by hand: nothing was paid, so there is nothing to refund. Use End now." }));
      const reason = String(form.get("reason") ?? "").trim().slice(0, 200) || "Refund requested by the customer";
      const result = await refundLast(env, id, reason);
      if ("error" in result) {
        await audit(db, "refund failed", id, result.error);
        return html(await dashboard(env, id, { error: result.error }));
      }
      await db.prepare("UPDATE subscriptions SET status = 'canceled', period_end = ?1 WHERE id = ?2").bind(now(), id).run();
      await audit(db, "refund", id, `${result.ok} Reason: ${reason}`);
      message = `${result.ok} Pro ended for this key.`;
      break;
    }
    case "end": {
      await db.prepare("UPDATE subscriptions SET status = 'canceled', period_end = ?1 WHERE id = ?2").bind(now(), id).run();
      await audit(db, "end", id, `was ${sub.status}, paid to ${date(sub.period_end)}`);
      message = paddle
        ? "Pro ended now. Cancel or refund it in Paddle too: Paddle's next event would turn it back on, and only Paddle moves money."
        : "Pro ended now.";
      break;
    }
    default: {
      // Deleting a live Paddle subscription here would leave Paddle charging for a license that no longer exists.
      if (paddle && sub.period_end > now() && sub.status !== "canceled") {
        return html(await dashboard(env, id, { error: `${id} is still live in Paddle. Cancel it in Paddle first, or use End to stop Pro now.` }));
      }
      await db.batch([
        db.prepare("DELETE FROM activations WHERE license_key = ?1").bind(sub.license_key),
        db.prepare("DELETE FROM subscriptions WHERE id = ?1").bind(id),
      ]);
      await audit(db, "delete", id, `${sub.plan ?? ""} ${sub.status}, ${sub.email ?? sub.customer_id}`);
      return html(await dashboard(env, "", { message: `Deleted ${id}. Its key no longer works.` }));
    }
  }
  return html(await dashboard(env, id, { message }));
}

// ---------- CSV ----------

/** One CSV cell, quoted; a value a spreadsheet would run as a formula is kept as text. */
function cell(v: unknown): string {
  let s = v === null || v === undefined ? "" : String(v);
  if (/^[=+\-@\t\r]/.test(s)) s = `'${s}`;
  return `"${s.replace(/"/g, '""')}"`;
}

async function exportCsv(db: D1Database, what: string): Promise<Response> {
  let rows: Row[];
  if (what === "installs") rows = await usersQuery(db, 0, 100000);
  else if (what === "subscriptions") rows = (await subsQuery(db).all<Row>()).results;
  else return new Response("Not found", { status: 404 });
  const cols = Object.keys(rows[0] ?? { empty: "" });
  const body = [cols, ...rows.map((r) => cols.map((c) => r[c]))].map((l) => l.map(cell).join(",")).join("\r\n") + "\r\n";
  return new Response(body, {
    headers: {
      "Content-Type": "text/csv; charset=utf-8",
      "Content-Disposition": `attachment; filename="deskmonk-${what}-${date(now())}.csv"`,
      "Cache-Control": "no-store",
    },
  });
}

// ---------- queries ----------

const subsQuery = (db: D1Database) =>
  db.prepare(
    `SELECT s.id, s.customer_id, s.email, s.plan, s.status, s.period_end, s.created_at, s.license_key,
            s.install_id, s.txn, s.ref, s.event_at, (SELECT COUNT(*) FROM activations a WHERE a.license_key = s.license_key) AS devices,
            (SELECT group_concat(a.install_id) FROM activations a WHERE a.license_key = s.license_key) AS device_ids,
            EXISTS (SELECT 1 FROM ignored_installs i WHERE i.install_id = s.install_id
                    OR i.install_id IN (SELECT a.install_id FROM activations a WHERE a.license_key = s.license_key)) AS mine
       FROM subscriptions s
      ORDER BY COALESCE(s.created_at, 0) DESC LIMIT 5000`,
  );

/** One row per installation seen since `since`, newest first. */
async function usersQuery(db: D1Database, since: number, limit: number): Promise<Row[]> {
  const latest = (col: string) =>
    `(SELECT x.${col} FROM events x WHERE x.install_id = e.install_id AND x.${col} IS NOT NULL ORDER BY x.ts DESC LIMIT 1)`;
  const { results } = await db
    .prepare(
      `SELECT e.install_id AS id, ${latest("os")} AS os, ${latest("country")} AS country, ${latest("version")} AS version,
              ${latest("plan")} AS plan, ${latest("channel")} AS channel, group_concat(DISTINCT e.client) AS client,
              (SELECT json_extract(s.value, '$.name') FROM hardware h, json_each(h.specs) s
                WHERE h.install_id = e.install_id AND json_extract(s.value, '$.kind') = 'İşlemci' LIMIT 1) AS cpu,
              (SELECT json_extract(s.value, '$.name') FROM hardware h, json_each(h.specs) s
                WHERE h.install_id = e.install_id AND json_extract(s.value, '$.kind') = 'Telefon' LIMIT 1) AS phone,
              (SELECT json_extract(s.value, '$.name') FROM hardware h, json_each(h.specs) s
                WHERE h.install_id = e.install_id AND json_extract(s.value, '$.kind') = 'Tür' LIMIT 1) AS type,
              MIN(e.ts) AS first_seen, MAX(e.ts) AS last_seen, SUM(e.kind = 'open') AS opens, SUM(e.kind = 'scan') AS scans,
              SUM(e.kind = 'clean') AS cleans, SUM(CASE WHEN e.kind = 'clean' THEN e.bytes ELSE 0 END) AS freed,
              SUM(e.kind = 'vault') AS vault, SUM(e.kind = 'focus') AS focus, SUM(e.kind = 'pro') AS pro_views,
              EXISTS (SELECT 1 FROM ignored_installs i WHERE i.install_id = e.install_id) AS mine
         FROM events e GROUP BY e.install_id HAVING MAX(e.ts) >= ?1 ORDER BY last_seen DESC LIMIT ?2`,
    )
    .bind(since, limit)
    .all<Row>();
  return results;
}

// ---------- dashboard ----------

async function dashboard(env: Env, query: string, notice: Notice): Promise<string> {
  const db = env.DB;
  const t = now();
  const today = Math.floor((t + TZ) / DAY) * DAY - TZ;
  const yesterday = today - DAY;
  const week = t - 7 * DAY;
  const month = t - 30 * DAY;
  const first = `f AS (SELECT install_id, MAX(os) AS os, MAX(type) AS type, MIN(ts) AS first, MAX(ts) AS last FROM ev GROUP BY install_id)`;
  const local = (col: string) => `date(${col} + ${TZ}, 'unixepoch')`;
  const spaceSums = `SUM(CASE WHEN kind = 'scan' THEN bytes END) AS found, SUM(CASE WHEN kind = 'clean' THEN bytes END) AS cleaned,
                  SUM(CASE WHEN kind = 'clean' THEN files END) AS files, SUM(kind = 'scan') AS scans,
                  SUM(CASE WHEN kind = 'scan' THEN duplicate_bytes END) AS dup, SUM(CASE WHEN kind = 'scan' THEN junk_bytes END) AS junk,
                  SUM(CASE WHEN kind = 'scan' THEN old_download_bytes END) AS old, SUM(CASE WHEN kind = 'scan' THEN large_bytes END) AS large`;
  const results = await db.batch([
    db.prepare(`WITH ${EV}, ${first} SELECT os, type, COUNT(*) AS total, SUM(first >= ?1) AS today, SUM(first >= ?2 AND first < ?1) AS yesterday,
                  SUM(first <= ?3) AS day_base, SUM(first <= ?3 AND last - first >= ${DAY}) AS day_back,
                  SUM(first <= ?4) AS week_base, SUM(first <= ?4 AND last - first >= ${7 * DAY}) AS week_back,
                  SUM(last >= ?5 AND last < ?4) AS lost FROM f GROUP BY os, type`).bind(today, yesterday, t - DAY, week, month),
    db.prepare(`WITH ${EV} SELECT os, type, COUNT(DISTINCT CASE WHEN ts >= ?1 THEN install_id END) AS today,
                  COUNT(DISTINCT CASE WHEN ts >= ?2 AND ts < ?1 THEN install_id END) AS yesterday,
                  COUNT(DISTINCT CASE WHEN ts >= ?3 THEN install_id END) AS week, COUNT(DISTINCT install_id) AS month,
                  SUM(CASE WHEN kind = 'clean' AND ts >= ?1 THEN bytes ELSE 0 END) AS cleaned_today,
                  SUM(CASE WHEN kind = 'clean' AND ts >= ?2 AND ts < ?1 THEN bytes ELSE 0 END) AS cleaned_yesterday
                FROM ev WHERE ts >= ?4 GROUP BY os, type`).bind(today, yesterday, week, month),
    db.prepare(`WITH ${EV}, ${first} SELECT ${local("first")} AS d, os, type, COUNT(*) AS n FROM f WHERE first >= ?1 GROUP BY d, os, type`).bind(today - 29 * DAY),
    // Plan and store of each installation's latest event (SQLite takes the bare columns from the MAX(ts) row).
    db.prepare(`WITH ${EV}, last AS (SELECT install_id, os, type, plan, channel, MAX(ts) FROM ev WHERE ts >= ?1 AND plan IS NOT NULL GROUP BY install_id)
                SELECT channel, plan, os, type, COUNT(*) AS n FROM last GROUP BY channel, plan, os, type`).bind(month),
    db.prepare(`WITH ${EV} SELECT kind, COALESCE(detail, '') AS detail, os, type, COUNT(*) AS n, COUNT(DISTINCT install_id) AS users,
                  SUM(ts >= ?2) AS week, SUM(ts >= ?3) AS today, SUM(ts >= ?4 AND ts < ?3) AS yesterday, SUM(bytes) AS bytes
                FROM ev WHERE ts >= ?1 GROUP BY kind, detail, os, type`).bind(month, week, today, yesterday),
    db.prepare(`WITH ${EV} SELECT COALESCE(country, '?') AS country, COUNT(DISTINCT CASE WHEN ts >= ?1 THEN install_id END) AS today,
                  COUNT(DISTINCT CASE WHEN ts >= ?2 THEN install_id END) AS week, COUNT(DISTINCT install_id) AS month,
                  COUNT(DISTINCT CASE WHEN plan = 'pro' THEN install_id END) AS pro
                FROM ev WHERE ts >= ?3 GROUP BY 1 ORDER BY month DESC, week DESC`).bind(today, week, month),
    db.prepare(`WITH ${EV} SELECT ${spaceSums} FROM ev`),
    // Each installation once, at the version of its latest event (bare columns come from the MAX(ts) row).
    db.prepare(`WITH ${EV}, cur AS (SELECT install_id, os, type, version, MAX(ts) FROM ev WHERE ts >= ?1 AND version IS NOT NULL GROUP BY install_id)
                SELECT version, os, type, COUNT(*) AS n FROM cur GROUP BY version, os, type`).bind(month),
    db.prepare(`WITH ${EV} SELECT client, COUNT(DISTINCT install_id) AS n FROM ev WHERE ts >= ?1 GROUP BY client ORDER BY n DESC`).bind(month),
    db.prepare(`WITH ${EV}, ${first} SELECT json_extract(s.value, '$.kind') AS kind, json_extract(s.value, '$.brand') AS brand, f.os, f.type, COUNT(DISTINCT h.install_id) AS n
                FROM hardware h LEFT JOIN f ON f.install_id = h.install_id, json_each(h.specs) s
                WHERE json_extract(s.value, '$.brand') != '' AND h.install_id NOT IN (SELECT install_id FROM ignored_installs)
                GROUP BY 1, 2, 3, 4`),
    db.prepare(`WITH ${EV} SELECT ${local("ts")} AS d, COUNT(DISTINCT install_id) AS active, SUM(kind = 'scan') AS scans, SUM(kind = 'clean') AS cleans,
                  SUM(CASE WHEN kind = 'clean' THEN bytes ELSE 0 END) AS freed, SUM(kind = 'vault') AS vault, SUM(kind = 'focus') AS focus,
                  SUM(kind = 'pro') AS pro_views FROM ev WHERE ts >= ?1 GROUP BY d ORDER BY d DESC`).bind(today - 29 * DAY),
    subsQuery(db),
    db.prepare("SELECT ts, action, ref, detail FROM admin_log ORDER BY id DESC LIMIT 60"),
    db.prepare(`WITH ${EV} SELECT COALESCE(country, '?') AS country, os, type, COUNT(DISTINCT install_id) AS n FROM ev WHERE ts >= ?1 GROUP BY 1, 2, 3`).bind(month),
    db.prepare(`WITH ${EV} SELECT os, type, ${spaceSums} FROM ev GROUP BY os, type`),
    db.prepare(`WITH ${EV} SELECT ${local("ts")} AS d, os, type, COUNT(DISTINCT install_id) AS n FROM ev WHERE ts >= ?1 GROUP BY 1, 2, 3`).bind(today - 29 * DAY),
    db.prepare(`SELECT source, SUM(kind = 'visit') AS visits, SUM(kind = 'download') AS downloads, SUM(kind = 'visit' AND ts >= ?2) AS today
                FROM visits WHERE ts >= ?1 GROUP BY source ORDER BY visits + downloads DESC LIMIT 40`).bind(month, today),
    db.prepare(`SELECT SUM(kind = 'visit' AND ts >= ?1) AS today, SUM(kind = 'visit' AND ts >= ?2 AND ts < ?1) AS yesterday,
                  SUM(kind = 'visit' AND ts >= ?3) AS week, SUM(kind = 'visit' AND ts >= ?4) AS month, SUM(kind = 'visit') AS total,
                  SUM(kind = 'download' AND ts >= ?1) AS downloads FROM visits`).bind(today, yesterday, week, month),
  ]);
  const [installs, active, series, plans, features, countries, space, versions, clients, brands, daily, subsAll, log, countryDevices, spaceDevices, dailyDevices, sources, [visitRow]] =
    results.map((r) => (r.results ?? []) as Row[]);
  // SUM over no rows is NULL.
  const visits = Object.fromEntries(["today", "yesterday", "week", "month", "total", "downloads"].map((k) => [k, Number(visitRow?.[k]) || 0]));
  const users = await usersQuery(db, month, 500);
  const [mobileNow, mobile] = await Promise.all([mobileLiveData(db, t), mobileData(db, t)]);

  const sum = (rows: Row[], f: string) => rows.reduce((a, r) => a + (Number(r[f]) || 0), 0);
  const featureRows = (kind: string, detail = "") => features.filter((r) => r.kind === kind && r.detail === detail);
  const feature = (kind: string, detail = "") => merge(featureRows(kind, detail));
  const featureAll = (kind: string) => features.filter((r) => r.kind === kind);

  // Subscriptions
  const live = (s: Row) => s.period_end > t && s.status !== "canceled";
  // Paying customers only: a subscription on a computer marked "This is me", or under the email of one, is yours.
  const ownEmails = new Set(subsAll.filter((s) => s.mine && s.email).map((s) => s.email));
  const paddleSubs = subsAll.filter((s) => s.customer_id !== "manual" && s.customer_id !== "pelamx" && !s.mine && !ownEmails.has(s.email));
  const livePaddle = paddleSubs.filter(live);
  const appstorePro = plans.filter((p) => p.channel === "appstore" && p.plan === "pro").reduce((a, p) => a + p.n, 0);
  const playstorePro = plans.filter((p) => p.channel === "playstore" && p.plan === "pro").reduce((a, p) => a + p.n, 0);
  const proInstalls = plans.filter((p) => p.plan === "pro").reduce((a, p) => a + p.n, 0);
  const freeInstalls = plans.filter((p) => p.plan === "free").reduce((a, p) => a + p.n, 0);
  const monthly = livePaddle.filter((s) => s.plan !== "yearly").length;
  const yearly = livePaddle.filter((s) => s.plan === "yearly").length;
  const mrr = monthly * PRICE.monthly + (yearly * PRICE.yearly) / 12;
  const ending = subsAll.filter((s) => live(s) && s.period_end <= t + 7 * DAY);
  const newPaddle = (from: number, to = Infinity) => paddleSubs.filter((s) => (s.created_at ?? 0) >= from && (s.created_at ?? 0) < to).length;
  const bought = featureAll("bought");
  const storeToday = sum(bought, "today");
  const storeYesterday = sum(bought, "yesterday");

  // Users and counts
  const real = users.filter((u) => !u.mine).length;
  const seriesMap: Record<string, number> = series.reduce((acc, r) => ({ ...acc, [r.d]: (acc[r.d] ?? 0) + r.n }), {});
  const days14 = [...Array(14)].map((_, i) => date(today + TZ - (13 - i) * DAY));
  const peak = Math.max(1, ...days14.map((d) => seriesMap[d] ?? 0));
  const seriesSplit = (d: string) => Object.entries(byPlatform(series.filter((r) => r.d === d), "n")).map(([k, n]) => `${platformName(k)} ${n}`).join(", ");
  const bars = days14
    .map((d, i) => `<div style="height:${Math.round(((seriesMap[d] ?? 0) / peak) * 100)}%" title="${d}: ${seriesMap[d] ?? 0}${i === 13 ? " (today)" : ""}${seriesMap[d] ? ` (${seriesSplit(d)})` : ""}"></div>`)
    .join("");
  const proStore = plans.filter((p) => p.plan === "pro" && STORE_NAME[p.channel]);
  const featureCols = deviceCols(features);
  const devicesOf = (k: string, rows: Row[], f = "n") => sum(rows.filter((r) => platform(r) === k), f);
  const storeLine = (os: string) => DEVICES.filter(([o]) => o === os).map(([o, type, name]) => `${name} ${devicesOf(`${o}:${type}`, proStore)}`).join(" · ");
  const brandRows = [...new Set(brands.map((b) => `${b.kind}\n${b.brand}`))]
    .map((key) => { const [kind, brand] = key.split("\n"); const rows = brands.filter((b) => b.kind === kind && b.brand === brand); return { kind, brand, rows, n: sum(rows, "n") }; })
    .sort((a, b) => a.kind.localeCompare(b.kind) || b.n - a.n);
  const brandCols = deviceCols(brands);
  const countryCols = deviceCols(countryDevices);
  const dailyCols = deviceCols(dailyDevices);
  const todayLabel = new Date((today + TZ) * 1000).toLocaleDateString("en-GB", { weekday: "long", day: "numeric", month: "long", timeZone: "UTC" });

  const q = query.toLowerCase();
  const matches = query
    ? subsAll.filter((s) =>
        [s.id, s.customer_id, s.email, s.license_key, s.install_id, s.txn, s.ref, s.device_ids].some((v) => typeof v === "string" && v.toLowerCase().includes(q)),
      )
    : subsAll;

  const nav = [
    ["overview", "grid", "Overview", ""],
    ["mobile", "phone", "Mobile", ""],
    ["analytics", "chart", "Analytics", ""],
    ["users", "users", "Users", count(real)],
    ["subscriptions", "key", "Subscriptions", ending.length ? count(ending.length, "warn") : count(subsAll.filter(live).length)],
    ["revenue", "wallet", "Revenue", ""],
    ["tools", "tools", "Tools", ""],
  ]
    .map(([id, ic, label, extra]) => `<a href="#${id}" title="${label}">${icon(ic)}<span class="nav-label">${label}</span>${extra}</a>`)
    .join("");

  const prod = env.PADDLE_ENV === "production";
  const body = `
  <header class="appbar">
    <a class="brand" href="/admin"><img src="/logo.svg" alt="">deskMonk</a><span class="tag">ADMIN</span>
    <span class="spacer"></span>
    <a class="btn" href="/admin/export.csv?what=installs" title="Installations CSV">${icon("file")}<span>Installs CSV</span></a>
    <a class="btn" href="/admin/export.csv?what=subscriptions" title="Subscriptions CSV">${icon("file")}<span>Subscriptions CSV</span></a>
    <a class="btn" href="/" title="deskmonk.app">${icon("globe")}<span>Site</span></a>
    <span class="env${prod ? " prod" : ""}" title="Paddle environment">Paddle ${esc(env.PADDLE_ENV ?? "sandbox")}</span>
  </header>
  <div class="shell">
  <nav class="side" aria-label="Sections">${nav}<p class="hint">Press <kbd>/</kbd> to search subscriptions</p></nav>
  <main class="main">
  ${notice.message ? `<p class="note ok">${esc(notice.message)}</p>` : ""}
  ${notice.error ? `<p class="note err">${esc(notice.error)}</p>` : ""}

  <div class="zone" id="overview">
  <section class="card">
    <p class="label">Today <span class="aside">· ${esc(todayLabel)} (Turkey time, vs yesterday)</span></p>
    <div class="grid">
      ${tile({ icon: "userPlus", tone: "t-blue", title: "New installs", hint: "Opened deskMonk for the first time",
        value: sum(installs, "today"), extra: delta(sum(installs, "today"), sum(installs, "yesterday")), rows: osBars(byPlatform(installs, "today")) })}
      ${tile({ icon: "users", tone: "t-teal", title: "Active users", hint: "Used deskMonk today",
        value: sum(active, "today"), extra: delta(sum(active, "today"), sum(active, "yesterday")), rows: osBars(byPlatform(active, "today")) })}
      ${tile({ icon: "broom", tone: "t-green", title: "Space freed", hint: "Cleaned by everyone today",
        value: humanSize(sum(active, "cleaned_today")), extra: deltaText(humanSize(sum(active, "cleaned_yesterday"))), rows: osBars(byPlatform(active, "cleaned_today"), humanSize) })}
      ${tile({ icon: "cart", tone: "t-red", title: "New Pro customers", hint: "Paddle + app stores",
        value: newPaddle(today) + storeToday, extra: delta(newPaddle(today) + storeToday, newPaddle(yesterday, today) + storeYesterday),
        rows: bars3([["Paddle", newPaddle(today), "web"], ...storeBars(bought, "today")]) })}
      ${tile({ icon: "globe", tone: "t-violet", title: "Website visits", hint: "deskmonk.app, bots left out",
        value: visits.today, extra: delta(visits.today, visits.yesterday),
        rows: bars3(sources.filter((r) => r.today).slice(0, 3).map((r): [string, number, string] => [esc(r.source), r.today, "web"])),
        foot: `${visits.downloads} downloads today · ${visits.week} visits in 7 days · ${visits.month} in 30 · ${visits.total} ever`, footIcon: "chart" })}
    </div>
    ${places(countries)}
  </section>

  <section class="card">
    <p class="label">Overall</p>
    <div class="grid five">
      ${tile({ icon: "star", tone: "t-orange", title: "Pro now", value: livePaddle.length + appstorePro + playstorePro, hint: "Paying: Paddle + app stores",
        rows: bars3([["Paddle", livePaddle.length, "web"], ...storeBars(proStore, "n")]),
        foot: `${monthly} monthly · ${yearly} yearly on Paddle`, footIcon: "calendar" })}
      ${tile({ icon: "bars", tone: "t-violet", title: "Active this week", value: sum(active, "week"), rows: osBars(byPlatform(active, "week")),
        foot: `${freeInstalls} free · ${proInstalls} Pro (last 30 days)`, footIcon: "users" })}
      ${tile({ icon: "download", tone: "t-blue", title: "Installs ever seen", value: sum(installs, "total"), rows: osBars(byPlatform(installs, "total")),
        foot: `${sum(active, "month")} active this month`, footIcon: "chart" })}
      ${tile({ icon: "clock", tone: "t-orange", title: "Ending in 7 days", value: ending.length, cls: ending.length ? "alert" : "",
        rows: bars3([["Paddle", ending.filter((s) => s.customer_id !== "manual").length, "web"], ["Manual", ending.filter((s) => s.customer_id === "manual").length, "manual"]]),
        foot: `${paddleSubs.filter((s) => s.status === "past_due").length} payment problems (past due)`, footIcon: "card" })}
    </div>
    <p class="label" style="margin-top:20px">New installs, last 14 days</p>
    <div class="bars">${bars}</div>
    <div class="bars-axis"><span>14 days ago</span><span>today</span></div>
  </section>
  </div>

  <div class="zone" id="mobile">${mobileLive(mobileNow, t)}${mobileCard(mobile, versions, t)}</div>

  <div class="zone" id="analytics">
  <section class="card">
    <p class="label">Features, last 30 days</p>
    <div class="scroll"><table>
      <tr><th>Feature</th><th class="num">Uses</th><th class="num">Users</th><th class="num">Last 7 days</th><th class="num">Today</th>${colHead(featureCols)}<th></th></tr>
      ${[
        ["App opened", featureRows("open"), ""],
        ["Scan", featureRows("scan"), `${humanSize(feature("scan").bytes ?? 0)} found`],
        ["Clean", featureRows("clean"), `${humanSize(feature("clean").bytes ?? 0)} freed`],
        ["Vault: encrypt", featureRows("vault", "encrypt"), ""],
        ["Vault: open", featureRows("vault", "open"), ""],
        ["Monk mode: focus finished", featureRows("focus"), ""],
        ["App lock: app locked", featureRows("applock", "lock"), ""],
        ["Pro dialog opened", featureAll("pro"), ""],
        ["App store purchase", bought, ""],
      ]
        .map(([label, rows, extra]: any) => { const r = merge(rows); return `<tr><td>${label}</td><td class="num">${r.n}</td><td class="num">${r.users}</td>
          <td class="num">${r.week}</td><td class="num">${r.today}</td>${colCells(featureCols, rows)}<td class="muted">${extra}</td></tr>`; })
        .join("")}
    </table></div>
    <p class="muted foot">Vault, Monk mode, Pro dialog and plan are reported by deskMonk 0.4 and later. Users = installations. Desktop and device columns count uses.</p>
  </section>

  <section class="card">
    <p class="label">Upgrade funnel, last 30 days</p>
    <div class="scroll"><table>
      <tr><th>What opened the Pro dialog</th><th class="num">Times</th><th class="num">Installs</th>${colHead(featureCols)}</tr>
      ${[
        ["⭐ Get Pro button", "button"], ["Top bar button (mobile)", "topbar"], ["Banner on Clean (mobile)", "clean_banner"], ["Daily clean limit (2 GB)", "clean_limit"],
        ["Vault limit (3 a day / 200 MB)", "vault_limit"], ["Vault size limit (mobile)", "vault_size"], ["App lock limit (1 app)", "applock_limit"],
      ]
        .map(([label, d]) => { const r = feature("pro", d); return `<tr><td>${label}</td><td class="num">${r.n}</td><td class="num">${r.users}</td>${colCells(featureCols, featureRows("pro", d))}</tr>`; })
        .join("")}
      <tr class="total"><td>Bought Pro (Paddle + app stores)</td><td class="num">${newPaddle(month) + sum(bought, "n")}</td>
        <td class="num">${ratio(newPaddle(month) + sum(bought, "n"), sum(featureAll("pro"), "users"))} of installs that saw the dialog</td>${colCells(featureCols, [...bought, { n: newPaddle(month) }])}</tr>
    </table></div>
  </section>

  <section class="card">
    <p class="label">Growth &amp; retention</p>
    <div class="scroll"><table>
      <tr><th>Platform</th><th class="num">Installs</th><th>Came back next day</th><th>Still using after a week</th><th class="num">Lost</th></tr>
      ${[...[...OSES, ...mobileKeys(installs)].map((k): Row => ({ ...totalRow(installs.filter((r) => platform(r) === k)), os: k })), totalRow(installs)]
        .map((r) => `<tr${r.os === "all" ? ' class="total"' : ""}><td>${platformName(r.os)}</td><td class="num">${r.total ?? 0}</td>
          <td>${ratio(r.day_back ?? 0, r.day_base ?? 0)}</td><td>${ratio(r.week_back ?? 0, r.week_base ?? 0)}</td><td class="num">${r.lost ?? 0}</td></tr>`)
        .join("")}
    </table></div>
    <p class="muted foot"><b>Came back / still using</b>: of the installs old enough to tell, how many were seen again at least a day / a week after their first event.
      <b>Lost</b>: used in the last 30 days, but not in the last 7.</p>

    <p class="label" style="margin-top:22px">Desktop versions in use, last 30 days <span class="aside">· each installation once, at the version it runs now</span></p>
    ${versionTable(versions, OSES, true)}
    <p class="muted foot"><b>Updates itself</b>: Macs now; Windows and Linux once their builds are listed in deskmonk.app/update.json.</p>
    <p class="muted foot">Interface: ${clients.map((c) => `${esc(c.client === "tui" ? "terminal" : c.client === "gui" ? "desktop" : c.client)} <b>${c.n}</b>`).join(" · ") || "—"}
      · Store: web ${sum(plans.filter((p) => p.channel === "web"), "n")} · App Store ${sum(plans.filter((p) => p.channel === "appstore"), "n")} · Google Play ${sum(plans.filter((p) => p.channel === "playstore"), "n")}</p>
  </section>

  <section class="card">
    <p class="label">Countries, last 30 days</p>
    <div class="scroll"><table>
      <tr><th>Country</th><th class="num">Installs</th><th class="num">Active 7 days</th><th class="num">Today</th><th class="num">Pro</th>${colHead(countryCols)}<th></th></tr>
      ${countries
        .map((c) => `<tr><td>${flag(c.country)} ${esc(countryName(c.country))}</td><td class="num">${c.month}</td><td class="num">${c.week}</td>
          <td class="num">${c.today}</td><td class="num">${c.pro}</td>${colCells(countryCols, countryDevices.filter((r) => r.country === c.country))}<td style="width:20%"><span class="track"><i style="width:${Math.round((c.month / Math.max(1, countries[0]?.month ?? 1)) * 100)}%"></i></span></td></tr>`)
        .join("") || `<tr><td colspan="${6 + countryCols.length}" class="muted">No countries yet: deskMonk 0.4 and later report them.</td></tr>`}
    </table></div>
  </section>

  <section class="card">
    <p class="label">Where website visitors come from, last 30 days</p>
    <div class="scroll"><table>
      <tr><th>Source</th><th class="num">Visits</th><th class="num">Today</th><th class="num">Downloads</th><th class="num">Downloads per visit</th><th></th></tr>
      ${sources
        .map((r) => `<tr><td>${esc(r.source)}</td><td class="num">${r.visits}</td><td class="num">${r.today}</td><td class="num">${r.downloads}</td>
          <td class="num">${r.visits ? `${Math.round((r.downloads / r.visits) * 100)}%` : "—"}</td><td style="width:20%"><span class="track"><i style="width:${Math.round((r.visits / Math.max(1, ...sources.map((x) => x.visits))) * 100)}%"></i></span></td></tr>`)
        .join("") || '<tr><td colspan="6" class="muted">No visits recorded yet.</td></tr>'}
    </table></div>
    <p class="muted foot"><b>Source</b>: the link's utm_source, a Google Ads click, else the site the visitor came from (any Google search is "google"); "direct" when there is none.
      Downloads are clicks on the site's download buttons, counted under the source of the page they were on. Your own visits are counted too.</p>
  </section>

  <section class="card">
    <p class="label">Space</p>
    <div class="grid">
      ${stat("Found by scans", humanSize(space[0]?.found ?? 0), `${space[0]?.scans ?? 0} scans`)}
      ${stat("Freed by cleaning", humanSize(space[0]?.cleaned ?? 0), `${space[0]?.files ?? 0} files`)}
      ${stat("Cleaned of what was found", space[0]?.found ? `${Math.round(((space[0]?.cleaned ?? 0) / space[0].found) * 100)}%` : "—", "all time")}
    </div>
    <p class="label" style="margin-top:20px">What scans find</p>
    ${hbars([["Duplicates", space[0]?.dup ?? 0], ["Junk files", space[0]?.junk ?? 0], ["Old downloads", space[0]?.old ?? 0], ["Large files", space[0]?.large ?? 0]])}
    <p class="label" style="margin-top:20px">By device</p>
    <div class="scroll"><table>
      <tr><th>Platform</th><th class="num">Scans</th><th class="num">Found</th><th class="num">Freed</th><th class="num">Files</th><th class="num">Duplicates</th><th class="num">Junk</th><th class="num">Old downloads</th><th class="num">Large</th></tr>
      ${[...OSES, ...mobileKeys(spaceDevices)]
        .map((k) => `<tr><td>${platformName(k)}</td><td class="num">${devicesOf(k, spaceDevices, "scans")}</td><td class="num">${humanSize(devicesOf(k, spaceDevices, "found"))}</td>
          <td class="num">${humanSize(devicesOf(k, spaceDevices, "cleaned"))}</td><td class="num">${devicesOf(k, spaceDevices, "files")}</td>
          ${["dup", "junk", "old", "large"].map((f) => `<td class="num">${humanSize(devicesOf(k, spaceDevices, f))}</td>`).join("")}</tr>`)
        .join("")}
    </table></div>
  </section>

  <section class="card">
    <p class="label">Hardware brands (latest known)</p>
    <div class="scroll"><table>
      <tr><th>Part</th><th>Brand</th><th class="num">Installs</th>${colHead(brandCols)}</tr>
      ${brandRows.map((b) => `<tr><td>${esc(KIND_LABEL[b.kind] ?? b.kind)}</td><td>${esc(b.brand)}</td><td class="num">${b.n}</td>${colCells(brandCols, b.rows)}</tr>`).join("") ||
        `<tr><td colspan="${3 + brandCols.length}" class="muted">No hardware reported yet.</td></tr>`}
    </table></div>
  </section>

  <details class="card">
    <summary>Day by day, last 30 days</summary>
    <div class="body scroll"><table>
      <tr><th>Day</th><th class="num">New</th><th class="num">Active</th>${colHead(dailyCols)}<th class="num">Scans</th><th class="num">Cleans</th><th class="num">Freed</th>
        <th class="num">Vault</th><th class="num">Focus</th><th class="num">Pro dialog</th></tr>
      ${daily
        .map((d) => `<tr><td>${d.d}</td><td class="num">${seriesMap[d.d] ?? 0}</td><td class="num">${d.active}</td>${colCells(dailyCols, dailyDevices.filter((r) => r.d === d.d))}<td class="num">${d.scans}</td>
          <td class="num">${d.cleans}</td><td class="num">${humanSize(d.freed)}</td><td class="num">${d.vault}</td><td class="num">${d.focus}</td>
          <td class="num">${d.pro_views}</td></tr>`)
        .join("") || `<tr><td colspan="${9 + dailyCols.length}" class="muted">Nothing this month.</td></tr>`}
    </table></div>
    <p class="muted foot">Desktop and device columns count active users.</p>
  </details>
  </div>

  <details class="card zone" id="users" open>
    <summary>Users, last 30 days: ${real}</summary>
    <div class="body">
      <p class="muted" style="margin-top:0">Real users only, one row per installation. Press <b>This is me</b> on your own machines, test runs and
        store reviewers to hide them here and in every count on this page.</p>
      ${userTable(users.filter((u) => !u.mine), "No real users this month.")}
    </div>
  </details>

  <div class="zone" id="subscriptions">
  ${ending.length ? `<details class="card warnbox" open><summary>Ending in 7 days (${ending.length})</summary><div class="body scroll"><table>
      <tr><th>Subscription</th><th>Email</th><th>Plan</th><th>Status</th><th>Ends</th></tr>
      ${ending.map((s) => `<tr><td><a href="/admin?q=${encodeURIComponent(s.id)}#subscriptions"><code>${esc(s.id)}</code></a></td><td>${esc(s.email ?? "—")}</td>
        <td>${planBadge(s)}</td><td>${statusBadge(s, t)}</td><td>${date(s.period_end)}</td></tr>`).join("")}
    </table></div></details>` : ""}
  <details class="card" id="subs" open>
    <summary>Subscriptions${query ? ` matching “${esc(query)}”` : ""}: ${matches.length}</summary>
    <div class="body">
      <form method="get" action="/admin" class="row">
        <div style="flex:1 1 280px"><label class="field" for="q">Search: email, license key, subscription id, install id</label>
          <input id="q" type="text" name="q" value="${esc(query)}" placeholder="DM-XXXX-XXXX-XXXX" spellcheck="false" autocomplete="off"></div>
        <button type="submit">Search</button>${query ? `<a class="btn-link" href="/admin#subscriptions">Clear</a>` : ""}
      </form>
      ${subGroup("Paddle", "paddle", matches.filter((s) => s.customer_id !== "manual"), t, !!query)}
      ${subGroup("Issued by hand", "manual", matches.filter((s) => s.customer_id === "manual"), t, !!query)}
      <p class="muted foot">Refund (under Manage) gives back a subscription's last payment and cancels it; Paddle's webhook updates this list. App Store and Google Play
        subscriptions belong to Apple and Google and appear only in the counts above (Pro installs on those versions).</p>
    </div>
  </details>
  </div>

  <section class="card zone" id="revenue">
    <p class="label">Revenue (Paddle, list prices, USD)</p>
    <div class="grid">
      ${stat("Monthly recurring", usd(mrr), `${monthly} × ${usd(PRICE.monthly)} + ${yearly} × ${usd(PRICE.yearly)}/12`)}
      ${stat("Yearly run rate", usd(mrr * 12), "MRR × 12")}
      ${stat("Live subscriptions", String(livePaddle.length), `${paddleSubs.filter((s) => s.status === "canceled").length} canceled all time`)}
      ${stat("App Store Pro installs", String(appstorePro), `${storeLine("ios")}. Apple pays out; see App Store Connect`)}
      ${stat("Google Play Pro installs", String(playstorePro), `${storeLine("android")}. Google pays out; see Play Console`)}
    </div>
    <p class="label" style="margin-top:20px">By month</p>
    <div class="scroll"><table>
      <tr><th>Month</th><th class="num">New monthly</th><th class="num">New yearly</th><th class="num">Canceled</th><th class="num">New list value</th></tr>
      ${months(paddleSubs, t)}
    </table></div>
    <p class="muted foot">Paddle takes its fee and sales tax before paying out; exact amounts are in Paddle → Reports.</p>
  </section>

  <div class="zone" id="tools">
  <details class="card">
    <summary>Issue a license</summary>
    <div class="body">
      <p class="muted">For people who never go through checkout: press, support, beta testers. It makes a working key with no payment
        behind it, marked <span class="badge manual">manual</span>; Paddle never changes it.</p>
      <form method="post" action="/admin/issue" class="row">
        <div style="flex:2 1 240px"><label class="field" for="issue-email">Email (for your records)</label>
          <input id="issue-email" type="email" name="email" placeholder="name@example.com" spellcheck="false"></div>
        <div style="flex:0 1 150px"><label class="field" for="issue-plan">Plan</label>
          <select id="issue-plan" name="plan"><option value="yearly">Yearly</option><option value="monthly">Monthly</option></select></div>
        <div style="flex:0 1 140px"><label class="field" for="issue-days">Days (empty: the plan's)</label>
          <input id="issue-days" type="number" name="days" min="1" max="3650" placeholder="365"></div>
        <button type="submit">Issue</button>
      </form>
    </div>
  </details>
  <details class="card">
    <summary>History (${log.length})</summary>
    <div class="body scroll"><table>
      <tr><th>When (UTC)</th><th>Action</th><th>Subscription</th><th>Detail</th></tr>
      ${log.map((l) => `<tr><td>${dateTime(l.ts)}</td><td>${esc(l.action)}</td><td>${l.ref ? `<a href="/admin?q=${encodeURIComponent(l.ref)}#subscriptions"><code>${esc(l.ref)}</code></a>` : ""}</td>
        <td class="muted">${esc(l.detail)}</td></tr>`).join("") || `<tr><td colspan="4" class="muted">Nothing done here yet.</td></tr>`}
    </table></div>
  </details>
  </div>
  </main>
  </div>
  <script>${SHELL_SCRIPT}</script>
  <script>setInterval(async () => { if (document.hidden) return; try { const r = await fetch("/admin/mobile-live"); if (r.ok) document.getElementById("mobile-live").outerHTML = await r.text(); } catch {} }, 30000);</script>`;
  return layout("Admin — deskMonk", body);
}

function merge(rows: Row[]): Row {
  return ["n", "users", "week", "today", "bytes"].reduce((acc, k) => ({ ...acc, [k]: rows.reduce((a, r) => a + (Number(r[k]) || 0), 0) }), {} as Row);
}

/** Each installation once, at its current version. `selfUpdates`: desktop only; the stores update the phone apps. */
function versionTable(versions: Row[], oses: string[], selfUpdates = false): string {
  const rows = versions.filter((r) => oses.includes(platform(r)));
  const n = (v: string, os?: string) => rows.filter((r) => r.version === v && (!os || platform(r) === os)).reduce((a, r) => a + r.n, 0);
  const all = Math.max(1, rows.reduce((a, r) => a + r.n, 0));
  return `<div class="scroll"><table>
      <tr><th>Version</th>${oses.map((os) => `<th class="num">${platformName(os)}</th>`).join("")}<th class="num">Total</th><th class="num">Share</th>${selfUpdates ? "<th>Updates itself</th>" : ""}</tr>
      ${[...new Set(rows.map((r) => r.version))].sort(compareVersions)
        .map((v, i) => {
          // The self-updater came with 0.5.0; older copies only update by a manual download.
          const updates = compareVersions(v, "0.5.0") <= 0 ? "yes" : '<span class="badge warn">no, download by hand</span>';
          return `<tr><td>${esc(v)}${i === 0 ? ' <span class="badge new">newest</span>' : ""}</td>${oses.map((os) => `<td class="num">${n(v, os) || '<span class="muted">0</span>'}</td>`).join("")}
            <td class="num"><b>${n(v)}</b></td><td class="num">${Math.round((n(v) / all) * 100)}%</td>${selfUpdates ? `<td>${updates}</td>` : ""}</tr>`;
        })
        .join("") || `<tr><td colspan="${oses.length + 3 + (selfUpdates ? 1 : 0)}" class="muted">No installs this month.</td></tr>`}
    </table></div>`;
}

// ---------- mobile ----------

interface MobileNow { counts: Row[]; recent: Row[]; devices: Row[] }
interface MobileData { lifecycle: Row[]; usage: Row[]; features: Row[]; active: Row[]; fresh: Row[]; phones: Row[]; countries: Row[]; devices: Row[] }

const DEVICES: [string, string, string][] = [["ios", "Phone", "iPhone"], ["ios", "Tablet", "iPad"], ["android", "Phone", "Android phone"], ["android", "Tablet", "Android tablet"]];

function deviceName(os: string, type: unknown): string {
  return DEVICES.find(([o, t]) => o === os && t === type)?.[2] ?? `${osName(os)}, type not reported`;
}

/** One row for each of the four devices, even at zero, plus installs that haven't said which they are. */
function deviceRows(rows: Row[]): Row[] {
  const known = (r: Row) => DEVICES.some(([o, t]) => o === r.os && t === r.type);
  return [...DEVICES.map(([os, type]) => rows.find((r) => r.os === os && r.type === type) ?? { os, type }), ...rows.filter((r) => !known(r))];
}

function deviceCell(r: Row): string {
  return `${osIcon(r.os)} ${esc(deviceName(r.os, r.type))}`;
}

const DEVICE_KEYS = DEVICES.map(([os, type]) => `${os}:${type}`);

/** Where a row of counts belongs: a desktop OS as it is, a phone or tablet as "os:type", with the type empty when not reported. */
function platform(r: Row): string {
  if (!MOBILE.includes(r.os)) return r.os;
  return `${r.os}:${DEVICE_KEYS.includes(`${r.os}:${r.type}`) ? r.type : ""}`;
}

function platformName(key: string): string {
  const [os, type] = key.split(":");
  return type === undefined ? osName(os) : deviceName(os, type);
}

/** The four devices, even at zero, then whichever mobile rows haven't said which they are. */
function mobileKeys(rows: Row[]): string[] {
  const unknown = rows.filter((r) => MOBILE.includes(r.os)).map(platform).filter((k) => !DEVICE_KEYS.includes(k));
  return [...DEVICE_KEYS, ...new Set(unknown)];
}

function byPlatform(rows: Row[], f: string): Record<string, number> {
  return rows.reduce((acc, r) => ({ ...acc, [platform(r)]: (acc[platform(r)] ?? 0) + (Number(r[f]) || 0) }), {});
}

/** Table columns split by device: all desktops as one, then each mobile device. */
function deviceCols(rows: Row[]): string[] {
  return ["desktop", ...mobileKeys(rows)];
}

function colHead(cols: string[]): string {
  return cols.map((k) => `<th class="num">${k === "desktop" ? "Desktop" : platformName(k)}</th>`).join("");
}

function colCells(cols: string[], rows: Row[], f = "n"): string {
  return cols
    .map((k) => {
      const n = rows.filter((r) => (MOBILE.includes(r.os) ? platform(r) : "desktop") === k).reduce((a, r) => a + (Number(r[f]) || 0), 0);
      return `<td class="num">${n || '<span class="muted">0</span>'}</td>`;
    })
    .join("");
}

/** Store purchases or Pro installs for bars3, by device; the Mac App Store shows only when it has any. */
function storeBars(rows: Row[], f: string): [string, number, string][] {
  const count = rows.reduce((acc, r) => ({ ...acc, [platform(r)]: (acc[platform(r)] ?? 0) + (Number(r[f]) || 0) }), {} as Record<string, number>);
  const mac = Object.entries(count).filter(([k]) => !k.includes(":")).reduce((a, [, n]) => a + n, 0);
  return [
    ...mobileKeys(rows).map((k): [string, number, string] => [platformName(k), count[k] ?? 0, k.startsWith("ios") ? "store" : "play"]),
    ...(mac ? [["Mac App Store", mac, "store"] as [string, number, string]] : []),
  ];
}

async function mobileLiveData(db: D1Database, t: number): Promise<MobileNow> {
  const today = Math.floor((t + TZ) / DAY) * DAY - TZ;
  const [counts, recent, devices] = (
    await db.batch([
      db.prepare(`WITH ${MO}, ${MO_F} SELECT os, SUM(last >= ?1) AS live, SUM(last >= ?2) AS hour, SUM(last >= ?3) AS today,
                    SUM(last >= ?4) AS week, SUM(last >= ?5) AS month FROM f GROUP BY os`).bind(t - LIVE, t - 3600, today, t - 7 * DAY, t - 30 * DAY),
      // country, version and plan are the bare columns of the MAX(ts) row: what the phone reported last.
      db.prepare(`WITH ${MO} SELECT install_id AS id, os, country, version, plan, MAX(ts) AS last_seen,
                    (SELECT x.kind FROM events x WHERE x.install_id = mo.install_id AND x.kind != 'ping' ORDER BY x.ts DESC LIMIT 1) AS action,
                    (SELECT json_extract(s.value, '$.name') FROM hardware h, json_each(h.specs) s
                      WHERE h.install_id = mo.install_id AND json_extract(s.value, '$.kind') = 'Telefon' LIMIT 1) AS phone, type
                  FROM mo GROUP BY install_id ORDER BY last_seen DESC LIMIT 15`),
      db.prepare(`WITH ${MO}, ${MO_F} SELECT os, type, SUM(last >= ?1) AS live, SUM(last >= ?2) AS hour, SUM(last >= ?3) AS today,
                    SUM(last >= ?4) AS week, SUM(last >= ?5) AS month FROM f GROUP BY 1, 2`).bind(t - LIVE, t - 3600, today, t - 7 * DAY, t - 30 * DAY),
    ])
  ).map((r) => (r.results ?? []) as Row[]);
  return { counts, recent, devices };
}

async function mobileData(db: D1Database, t: number): Promise<MobileData> {
  const today = Math.floor((t + TZ) / DAY) * DAY - TZ;
  const month = t - 30 * DAY;
  const day = (col: string) => `date(${col} + ${TZ}, 'unixepoch')`;
  const back = (name: string, age: number, ago: number) =>
    `SUM(first <= ${ago}) AS ${name}_base, SUM(first <= ${ago} AND last - first >= ${age}) AS ${name}_back`;
  const seen = (kind: string) => `COUNT(DISTINCT CASE WHEN kind = '${kind}' THEN install_id END)`;
  const [lifecycle, usage, features, active, fresh, phones, countries, devices] = (
    await db.batch([
      db.prepare(`WITH ${MO}, ${MO_F} SELECT os, type, COUNT(*) AS total, SUM(first >= ?1) AS new_today, SUM(first >= ?2 AND first < ?1) AS new_yesterday,
                    SUM(last >= ?1) AS today, SUM(last >= ?4) AS month,
                    ${back("day", DAY, t - DAY)}, ${back("week", 7 * DAY, t - 7 * DAY)}, ${back("month", 30 * DAY, month)},
                    SUM(last >= ?4 AND last < ?3) AS dormant, SUM(last < ?4) AS gone
                  FROM f GROUP BY os, type`).bind(today, today - DAY, t - 7 * DAY, month),
      // Pro is each installation's latest plan. Everyone sends a basic open; anything else means they allowed the full stats.
      db.prepare(`WITH ${MO}, last AS (SELECT install_id, os, type, plan, MAX(ts) FROM mo WHERE ts >= ?1 AND plan IS NOT NULL GROUP BY install_id)
                  SELECT os, type, COUNT(DISTINCT install_id) AS active,
                    COUNT(DISTINCT CASE WHEN kind != 'open' OR COALESCE(detail, '') != 'basic' THEN install_id END) AS allowed,
                    ${seen("scan")} AS scanned, ${seen("clean")} AS cleaned, ${seen("pro")} AS saw_pro, ${seen("bought")} AS bought,
                    (SELECT COUNT(*) FROM last WHERE last.os = mo.os AND last.type IS mo.type AND last.plan = 'pro') AS pro
                  FROM mo WHERE ts >= ?1 GROUP BY os, type`).bind(month),
      db.prepare(`WITH ${MO} SELECT os, type, kind, COALESCE(detail, '') AS detail, COUNT(*) AS n, COUNT(DISTINCT install_id) AS users, SUM(bytes) AS bytes
                  FROM mo WHERE ts >= ?1 AND kind != 'ping' GROUP BY 1, 2, 3, 4`).bind(month),
      db.prepare(`WITH ${MO} SELECT ${day("ts")} AS d, os, type, COUNT(DISTINCT install_id) AS n FROM mo WHERE ts >= ?1 GROUP BY 1, 2, 3`).bind(today - 29 * DAY),
      db.prepare(`WITH ${MO}, ${MO_F} SELECT ${day("first")} AS d, os, type, COUNT(*) AS n FROM f WHERE first >= ?1 GROUP BY 1, 2, 3`).bind(today - 29 * DAY),
      db.prepare(`WITH ${MO} SELECT json_extract(s.value, '$.name') AS name, COUNT(DISTINCT h.install_id) AS n FROM hardware h, json_each(h.specs) s
                  WHERE json_extract(s.value, '$.kind') = 'Telefon' AND h.install_id IN (SELECT install_id FROM mo) GROUP BY 1 ORDER BY n DESC LIMIT 8`),
      db.prepare(`WITH ${MO} SELECT COALESCE(country, '?') AS country, COUNT(DISTINCT install_id) AS n FROM mo WHERE ts >= ?1 GROUP BY 1 ORDER BY n DESC LIMIT 8`).bind(month),
      // The four devices side by side. Usage is the last 30 days; Pro is each installation's latest plan.
      db.prepare(`WITH ${MO}, ${MO_F},
                    p AS (SELECT install_id, plan, MAX(ts) FROM mo WHERE plan IS NOT NULL GROUP BY install_id),
                    u AS (SELECT install_id, MAX(kind = 'scan') AS scanned, MAX(kind = 'clean') AS cleaned, MAX(kind = 'vault') AS vaulted, MAX(kind = 'focus') AS focused, MAX(kind = 'bought') AS bought
                      FROM mo WHERE ts >= ?1 GROUP BY install_id)
                  SELECT f.os, f.type, COUNT(*) AS total, SUM(first >= ?2) AS new_today, SUM(last >= ?1) AS active, SUM(p.plan = 'pro') AS pro,
                    SUM(u.scanned) AS scanned, SUM(u.cleaned) AS cleaned, SUM(u.vaulted) AS vaulted, SUM(u.focused) AS focused, SUM(u.bought) AS bought
                  FROM f LEFT JOIN p ON p.install_id = f.install_id LEFT JOIN u ON u.install_id = f.install_id GROUP BY 1, 2`).bind(month, today),
    ])
  ).map((r) => (r.results ?? []) as Row[]);
  return { lifecycle, usage, features, active, fresh, phones, countries, devices };
}

const ACTION: Record<string, string> = { open: "Opened the app", scan: "Scanned", clean: "Cleaned", vault: "Used the Vault", focus: "Finished a focus", pro: "Looked at Pro", bought: "Bought Pro", applock: "Locked an app" };

function liveness(last: number, t: number): [string, string] {
  const s = t - last;
  if (s < LIVE) return ["live", "Live"];
  if (s < DAY) return ["recent", "Today"];
  if (s < 7 * DAY) return ["idle", "This week"];
  if (s < 30 * DAY) return ["dormant", "Dormant"];
  return ["gone", "Gone"];
}

/** Who is on the app right now and who was lately. The page refetches this part every 30 seconds. */
export function mobileLive({ counts, recent, devices }: MobileNow, t: number): string {
  const total = (f: string) => counts.reduce((a, r) => a + (Number(r[f]) || 0), 0);
  const box = (f: string, icon: string, tone: string, title: string, hint: string, cls = "") =>
    tile({ icon, tone, title, hint, cls, value: total(f), rows: osBars(byPlatform(devices, f), String, DEVICE_KEYS) });
  const clock = new Date((t + TZ) * 1000).toISOString().slice(11, 16);
  return `<section class="card" id="mobile-live">
    <p class="label"><span class="dot live"></span>Phone and tablet users, live <span class="aside">· iOS and Android · updated ${clock} Turkey time, refreshes by itself</span></p>
    <div class="grid five">
      ${box("live", "phone", "t-green", "Live now", "App on screen, last 5 minutes", "live")}
      ${box("hour", "clock", "t-teal", "Last hour", "Used the app")}
      ${box("today", "users", "t-blue", "Today", "Daily active")}
      ${box("week", "bars", "t-violet", "This week", "Weekly active")}
      ${box("month", "chart", "t-orange", "This month", "Monthly active")}
    </div>
    <p class="label" style="margin-top:20px">By device</p>
    <div class="scroll"><table>
      <tr><th>Device</th><th class="num">Live now</th><th class="num">Last hour</th><th class="num">Today</th><th class="num">This week</th><th class="num">This month</th></tr>
      ${deviceRows(devices)
        .map((d) => `<tr><td>${deviceCell(d)}</td>${["live", "hour", "today", "week", "month"].map((f) => `<td class="num">${d[f] ?? 0}</td>`).join("")}</tr>`)
        .join("")}
    </table></div>
    <p class="label" style="margin-top:20px">Latest seen</p>
    <div class="scroll"><table>
      <tr><th>Status</th><th>Installation</th><th>Device</th><th>Where</th><th>Version</th><th>Plan</th><th>Last seen</th><th>Last did</th></tr>
      ${recent
        .map((u) => {
          const [cls, label] = liveness(u.last_seen, t);
          return `<tr><td><span class="dot ${cls}"></span>${label}</td><td><a href="/admin/user/${encodeURIComponent(u.id)}"><code>${esc(String(u.id).slice(0, 8))}</code></a></td>
            <td>${deviceCell(u)}${u.phone ? ` <span class="muted">· ${esc(u.phone)}</span>` : ""}</td><td>${u.country ? `${flag(u.country)} ${esc(u.country)}` : '<span class="muted">—</span>'}</td>
            <td>${esc(u.version ?? "")}</td><td>${u.plan === "pro" ? '<span class="badge on">Pro</span>' : '<span class="badge">Free</span>'}</td>
            <td>${ago(u.last_seen)}</td><td class="muted">${ACTION[u.action] ?? "—"}</td></tr>`;
        })
        .join("") || `<tr><td colspan="8" class="muted">No phone or tablet has reported yet.</td></tr>`}
    </table></div>
  </section>`;
}

/** Bars per day, the mobile devices stacked. */
function dayStack(rows: Row[], days: string[]): string {
  const keys = mobileKeys(rows);
  const n = (d: string, k: string) => rows.filter((r) => r.d === d && platform(r) === k).reduce((a, r) => a + (Number(r.n) || 0), 0);
  const peak = Math.max(1, ...days.map((d) => keys.reduce((a, k) => a + n(d, k), 0)));
  return `<div class="stack">${days
    .map((d) => `<span title="${d}: ${keys.map((k) => `${platformName(k)} ${n(d, k)}`).join(", ")}">${keys.map((k) => `<i class="${k.replace(":", "-")}" style="height:${Math.round((n(d, k) / peak) * 100)}%"></i>`).join("")}</span>`)
    .join("")}</div><div class="bars-axis"><span>${days.length} days ago</span><span>today</span></div>`;
}

const STACK_LEGEND = `<span class="legend">${DEVICE_KEYS.map((k) => `<span><i class="${k.replace(":", "-")}"></i>${platformName(k)}</span>`).join("")}</span>`;

/** What the phones do with the app: growth, retention, what they use and what leads to Pro. */
export function mobileCard(m: MobileData, versions: Row[], t: number): string {
  const keys = mobileKeys([...m.usage, ...m.lifecycle]);
  const rows: Row[] = [...keys, "all"].map((key) => {
    const merged = (list: Row[]) =>
      list.filter((r) => key === "all" || platform(r) === key).reduce((acc, r) => Object.fromEntries(Object.keys(r).map((k) => [k, (Number(acc[k]) || 0) + (Number(r[k]) || 0)])), {} as Row);
    return { ...merged(m.usage), ...merged(m.lifecycle), key };
  });
  const all = rows[rows.length - 1];
  const num = (v: unknown) => `<td class="num">${v ?? 0}</td>`;
  const keyHead = keys.map((k) => `<th class="num">${platformName(k)}</th>`).join("");
  const today = Math.floor((t + TZ) / DAY) * DAY;
  const days = [...Array(30)].map((_, i) => date(today - (29 - i) * DAY));
  const uses = (kind: string, detail?: string) => m.features.filter((f) => f.kind === kind && (detail === undefined || f.detail === detail));
  const sum = (list: Row[], f: string) => list.reduce((a, r) => a + (Number(r[f]) || 0), 0);
  const freed = sum(m.features.filter((f) => f.kind === "clean"), "bytes");
  const feature = (label: string, kind: string, detail?: string) => {
    const list = uses(kind, detail);
    return `<tr><td>${label}</td>${keys.map((k) => num(sum(list.filter((f) => platform(f) === k), "n"))).join("")}${num(sum(list, "n"))}</tr>`;
  };
  const funnel = [["Used the app and allowed stats", "allowed"], ["Scanned", "scanned"], ["Cleaned", "cleaned"], ["Looked at Pro", "saw_pro"], ["Bought Pro", "bought"]];
  const sources = Object.entries(
    m.features.filter((f) => f.kind === "pro").reduce((acc, f) => ({ ...acc, [f.detail]: (acc[f.detail] ?? 0) + f.n }), {} as Record<string, number>),
  ).sort((a, b) => b[1] - a[1]);
  const SOURCE: Record<string, string> = { topbar: "Top bar button", clean_banner: "Banner on Clean", clean_limit: "Daily clean limit", vault_limit: "Daily Vault limit", vault_size: "Vault size limit", applock_limit: "App lock limit (1 app)" };
  return `<section class="card">
    <p class="label">Mobile analytics <span class="aside">· last 30 days unless it says otherwise</span></p>
    <div class="grid five">
      ${stat("New installs today", String(all.new_today ?? 0), `${all.new_yesterday ?? 0} yesterday`)}
      ${stat("Stickiness", all.month ? `${Math.round(((all.today ?? 0) / all.month) * 100)}%` : "—", "Active today of active this month")}
      ${stat("Space freed", humanSize(freed), `Cleaned ${sum(m.features.filter((f) => f.kind === "clean"), "n")} times`)}
      ${stat("Pro now", String(all.pro ?? 0), `${all.bought ?? 0} bought this month`)}
      ${stat("Allowed stats", all.active ? `${Math.round(((all.allowed ?? 0) / all.active) * 100)}%` : "—", "The rest count only when the app starts")}
    </div>
    <div class="two">
      <div><p class="label">Active users per day ${STACK_LEGEND}</p>${dayStack(m.active, days)}</div>
      <div><p class="label">New installs per day ${STACK_LEGEND}</p>${dayStack(m.fresh, days)}</div>
    </div>
    <p class="label" style="margin-top:22px">Liveness by device <span class="aside">· all time</span></p>
    <div class="scroll"><table>
      <tr><th>Device</th><th class="num">Installs</th><th>Came back next day</th><th>Still using after a week</th><th>After a month</th><th class="num">Dormant</th><th class="num">Gone</th></tr>
      ${rows
        .map((r) => `<tr${r.key === "all" ? ' class="total"' : ""}><td>${r.key === "all" ? "" : osIcon(r.key.split(":")[0])} ${platformName(r.key)}</td>${num(r.total)}
          <td>${ratio(r.day_back ?? 0, r.day_base ?? 0)}</td><td>${ratio(r.week_back ?? 0, r.week_base ?? 0)}</td><td>${ratio(r.month_back ?? 0, r.month_base ?? 0)}</td>${num(r.dormant)}${num(r.gone)}</tr>`)
        .join("")}
    </table></div>
    <p class="muted foot"><b>Dormant</b>: last seen 7 to 30 days ago. <b>Gone</b>: not seen for over 30 days; a phone or tablet never says it uninstalled the app.</p>
    <p class="label" style="margin-top:22px">By device <span class="aside">· installs and Pro all time, the rest last 30 days</span></p>
    <div class="scroll"><table>
      <tr><th>Device</th><th class="num">Installs</th><th class="num">New today</th><th class="num">Active</th><th class="num">Pro now</th><th class="num">Scanned</th><th class="num">Cleaned</th><th class="num">Vault</th><th class="num">Focus</th><th class="num">Bought</th></tr>
      ${deviceRows(m.devices)
        .map((d) => `<tr><td>${deviceCell(d)}</td>${["total", "new_today", "active", "pro", "scanned", "cleaned", "vaulted", "focused", "bought"].map((f) => num(d[f])).join("")}</tr>`)
        .join("")}
    </table></div>
    <p class="muted foot">The type comes from users who allowed the stats. The others appear as "type not reported".</p>
    <div class="two">
      <div><p class="label">From first use to Pro</p><div class="scroll"><table>
        <tr><th>Step</th>${keyHead}<th class="num">Total</th><th>Of previous step</th></tr>
        ${funnel
          .map(([label, k], i) => `<tr><td>${label}</td>${rows.slice(0, -1).map((r) => num(r[k])).join("")}${num(all[k])}<td>${i ? ratio(all[k] ?? 0, all[funnel[i - 1][1]] ?? 0) : ""}</td></tr>`)
          .join("")}
      </table></div></div>
      <div><p class="label">Features</p><div class="scroll"><table>
        <tr><th>Feature</th>${keyHead}<th class="num">Total</th></tr>
        ${feature("App opened", "open")}${feature("Scan", "scan")}${feature("Clean", "clean")}${feature("Vault: encrypt", "vault", "encrypt")}${feature("Vault: open", "vault", "open")}
        ${feature("Focus finished", "focus")}${feature("App lock: app locked", "applock", "lock")}${feature("Pro sheet", "pro")}${feature("Purchase", "bought")}
      </table></div></div>
    </div>
    <div class="two">
      <div><p class="label">What opened the Pro sheet</p>${sources.length ? hbars(sources.map(([d, n]) => [SOURCE[d] ?? esc(d || "—"), n]), String) : '<p class="muted">Nothing yet.</p>'}</div>
      <div><p class="label">Device models</p>${m.phones.length ? hbars(m.phones.map((p) => [esc(p.name), p.n]), String) : '<p class="muted">No phone or tablet has reported yet.</p>'}</div>
      <div><p class="label">Countries</p>${m.countries.length ? hbars(m.countries.map((c) => [`${flag(c.country)} ${esc(countryName(c.country))}`, c.n]), String) : '<p class="muted">Nothing yet.</p>'}</div>
    </div>
    <p class="label" style="margin-top:22px">Mobile versions in use</p>
    ${versionTable(versions, mobileKeys(versions))}
    <p class="muted foot">Counted from the apps' own events. Money and refunds are in App Store Connect and Google Play Console.</p>
  </section>`;
}

function totalRow(rows: Row[]): Row {
  const keys = ["total", "day_base", "day_back", "week_base", "week_back", "lost"];
  return { os: "all", ...Object.fromEntries(keys.map((k) => [k, rows.reduce((a, r) => a + (Number(r[k]) || 0), 0)])) };
}

function subGroup(title: string, cls: string, subs: Row[], t: number, open: boolean): string {
  if (subs.length === 0) return `<div class="group ${cls}"><p class="group-head"><b>${title}</b> <span class="muted">none</span></p></div>`;
  return `<div class="group ${cls}"><p class="group-head"><b>${title}</b> <span class="muted">${subs.length}</span></p>
    <div class="scroll"><table>
      <tr><th>Subscription</th><th>Email</th><th>Plan</th><th>Status</th><th>Started</th><th>Paid to</th><th>Devices</th><th>Key</th><th></th></tr>
      ${subs
        .map((s) => `<tr><td><code>${esc(s.id)}</code></td><td>${esc(s.email ?? "—")}</td><td>${planBadge(s)}</td><td>${statusBadge(s, t)}</td>
          <td>${s.created_at ? date(s.created_at) : "—"}</td><td>${date(s.period_end)}</td>
          <td>${s.devices}/${MAX_DEVICES} ${(s.device_ids ?? "").split(",").filter(Boolean).map((d: string) => `<a href="/admin/user/${encodeURIComponent(d)}" title="${esc(d)}">${esc(d.slice(0, 8))}</a>`).join(" ")}</td>
          <td><code class="k">${esc(s.license_key)}</code></td>
          <td><details class="act"${open ? " open" : ""}><summary>Manage</summary>${actions(s)}</details></td></tr>`)
        .join("")}
    </table></div></div>`;
}

function actions(s: Row): string {
  const id = `<input type="hidden" name="id" value="${esc(s.id)}">`;
  const confirm = (msg: string) => `onsubmit="return confirm('${msg}')"`;
  return `<div class="acts">
    <form method="post" action="/admin/extend" class="inline">${id}<input type="number" name="days" min="1" max="3650" value="30" aria-label="Days"><button>Extend days</button></form>
    <form method="post" action="/admin/devices" class="inline" ${confirm("Free all device slots of this key?")}>${id}<button class="quiet">Free devices</button></form>
    <form method="post" action="/admin/rotate" class="inline" ${confirm("Make a new key? The old key stops working on every computer.")}>${id}<button class="quiet">New key</button></form>
    ${s.customer_id !== "manual" ? `<form method="post" action="/admin/refund" class="inline" ${confirm("Refund the last payment in full, cancel the subscription and end Pro now? The money goes back to the buyer.")}>${id}<input type="text" name="reason" maxlength="200" placeholder="Reason (for Paddle)" aria-label="Refund reason"><button class="danger">Refund</button></form>` : ""}
    <form method="post" action="/admin/end" class="inline" ${confirm("End Pro now for this subscription?")}>${id}<button class="quiet">End now</button></form>
    <form method="post" action="/admin/delete" class="inline" ${confirm("Delete this subscription and its key for good?")}>${id}<button class="danger">Delete</button></form>
  </div>`;
}

function months(subs: Row[], t: number): string {
  const rows = [...Array(6)].map((_, i) => {
    const d = new Date(t * 1000);
    d.setUTCDate(1);
    d.setUTCMonth(d.getUTCMonth() - i);
    const ym = d.toISOString().slice(0, 7);
    const made = subs.filter((s) => s.created_at && date(s.created_at).startsWith(ym));
    const m = made.filter((s) => s.plan !== "yearly").length;
    const y = made.filter((s) => s.plan === "yearly").length;
    const canceled = subs.filter((s) => s.status === "canceled" && String(s.event_at ?? "").startsWith(ym)).length;
    return `<tr><td>${ym}</td><td class="num">${m}</td><td class="num">${y}</td><td class="num">${canceled}</td><td class="num">${usd(m * PRICE.monthly + y * PRICE.yearly)}</td></tr>`;
  });
  return rows.join("");
}

/** The installations table: one row each, `empty` when there are none. */
function userTable(rows: Row[], empty: string): string {
  return `<div class="scroll"><table>
    <tr><th>Install</th><th>Where</th><th>System</th><th>Plan</th><th>Version</th><th>First seen</th><th>Last seen</th>
      <th class="num">Opens</th><th class="num">Scans</th><th class="num">Cleans</th><th class="num">Freed</th><th class="num">Vault</th><th class="num">Focus</th><th></th></tr>
    ${rows.map((u) => userRow(u, "/admin#users")).join("") || `<tr><td colspan="14" class="muted">${empty}</td></tr>`}
  </table></div>`;
}

function userRow(u: Row, back: string): string {
  const tags = [u.mine ? `<span class="badge you">you</span>` : "", STORE_NAME[u.channel] ? `<span class="badge store">${STORE_NAME[u.channel]}</span>` : ""].join(" ");
  return `<tr${u.mine ? ' class="mine"' : ""}><td><a href="/admin/user/${encodeURIComponent(u.id)}"><code>${esc(String(u.id).slice(0, 8))}</code></a> ${tags}</td>
    <td>${u.country ? `${flag(u.country)} ${esc(u.country)}` : '<span class="muted">—</span>'}</td>
    <td>${osIcon(u.os)} ${esc(MOBILE.includes(u.os) ? [deviceName(u.os, u.type), u.phone].filter(Boolean).join(" · ") : (u.cpu ?? osName(u.os)))}${u.client && u.client !== "gui" ? ` <span class="muted">(${u.client === "tui" ? "terminal" : esc(u.client)})</span>` : ""}</td>
    <td>${u.plan === "pro" ? '<span class="badge on">Pro</span>' : u.plan === "free" ? '<span class="badge">Free</span>' : '<span class="muted">—</span>'}</td>
    <td>${esc(u.version ?? "")}</td><td>${date(u.first_seen)}</td><td>${ago(u.last_seen)}</td>
    <td class="num">${u.opens}</td><td class="num">${u.scans}</td><td class="num">${u.cleans}</td><td class="num">${humanSize(u.freed)}</td>
    <td class="num">${u.vault}</td><td class="num">${u.focus}</td>
    <td><form method="post" action="/admin/mine" class="inline"><input type="hidden" name="install" value="${esc(u.id)}">
      <input type="hidden" name="mine" value="${u.mine ? 0 : 1}"><input type="hidden" name="back" value="${esc(back)}">
      <button class="quiet">${u.mine ? "Not me" : "This is me"}</button></form></td></tr>`;
}

// ---------- one installation ----------

async function userPage(db: D1Database, id: string): Promise<string> {
  const [summary, events, subs] = (
    await db.batch([
      db.prepare(`SELECT SUM(kind = 'open') AS opens, SUM(kind = 'scan') AS scans, SUM(kind = 'clean') AS cleans,
                    SUM(CASE WHEN kind = 'clean' THEN bytes END) AS freed, SUM(CASE WHEN kind = 'scan' THEN bytes END) AS found,
                    SUM(kind = 'vault') AS vault, SUM(kind = 'focus') AS focus, SUM(kind = 'pro') AS pro_views,
                    MIN(ts) AS first_seen, MAX(ts) AS last_seen FROM events WHERE install_id = ?1`).bind(id),
      db.prepare(`SELECT ts, kind, detail, client, os, version, country, plan, channel, files, bytes FROM events WHERE install_id = ?1 AND kind != 'ping' ORDER BY ts DESC LIMIT 1000`).bind(id),
      db.prepare(`SELECT s.id, s.plan, s.status, s.period_end, s.license_key, s.customer_id FROM subscriptions s
                  WHERE s.install_id = ?1 OR s.license_key IN (SELECT license_key FROM activations WHERE install_id = ?1)`).bind(id),
    ])
  ).map((r) => (r.results ?? []) as Row[]);
  const u = (await usersQuery(db, 0, 100000)).find((r) => r.id === id);
  const s = summary[0] ?? {};
  const t = now();
  let body = `<header class="appbar"><a class="brand" href="/admin"><img src="/logo.svg" alt="">deskMonk</a><span class="tag">ADMIN</span></header>
  <main class="main solo">
  <p><a href="/admin#users">← All users</a></p>
  <h1 class="title">Installation <code>${esc(id)}</code> ${u?.mine ? '<span class="badge you">you</span>' : ""}</h1>
  <p class="muted">${u ? `${u.country ? `${flag(u.country)} ${esc(countryName(u.country))} · ` : ""}${osName(u.os)} · ${esc(u.version ?? "")} ·
    ${u.plan === "pro" ? "Pro" : "Free"}${STORE_NAME[u.channel] ? ` (${STORE_NAME[u.channel]})` : ""} · first seen ${date(u.first_seen)} · last seen ${ago(u.last_seen)}` : "No events from this installation."}</p>
  ${u ? `<form method="post" action="/admin/mine" class="inline"><input type="hidden" name="install" value="${esc(id)}"><input type="hidden" name="mine" value="${u.mine ? 0 : 1}">
    <input type="hidden" name="back" value="/admin/user/${esc(encodeURIComponent(id))}"><button class="quiet">${u.mine ? "Not me" : "This is me"}</button></form>` : ""}
  <section class="card" style="margin-top:16px"><div class="grid">
    ${stat("App opened", String(s.opens ?? 0))}${stat("Scans", String(s.scans ?? 0), `${humanSize(s.found ?? 0)} found`)}
    ${stat("Cleans", String(s.cleans ?? 0), `${humanSize(s.freed ?? 0)} freed`)}${stat("Vault", String(s.vault ?? 0))}
    ${stat("Focus sessions", String(s.focus ?? 0))}${stat("Pro dialog", String(s.pro_views ?? 0))}
  </div></section>`;
  if (subs.length) {
    body += `<section class="card"><p class="label">Licenses on this installation</p><div class="scroll"><table>
      <tr><th>Subscription</th><th>Plan</th><th>Status</th><th>Paid to</th><th>Key</th></tr>
      ${subs.map((x) => `<tr><td><a href="/admin?q=${encodeURIComponent(x.id)}#subscriptions"><code>${esc(x.id)}</code></a></td><td>${planBadge(x)}</td>
        <td>${statusBadge(x, t)}</td><td>${date(x.period_end)}</td><td><code class="k">${esc(x.license_key)}</code></td></tr>`).join("")}
    </table></div></section>`;
  }
  const hw = await db.prepare("SELECT ts, specs FROM hardware WHERE install_id = ?1").bind(id).first<{ ts: number; specs: string }>();
  body += hw ? hardwareHtml(dateTime(hw.ts), hw.specs) : `<section class="card"><p class="muted">No hardware reported yet.</p></section>`;
  body += `<section class="card"><p class="label">Events (${events.length}${events.length === 1000 ? ", newest" : ""})</p><div class="scroll"><table>
    <tr><th>When (UTC)</th><th>Event</th><th>Detail</th><th>Interface</th><th>Version</th><th>Plan</th><th>Where</th><th class="num">Files</th><th class="num">Size</th></tr>
    ${events.map((e) => `<tr><td>${dateTime(e.ts)}</td><td>${esc(e.kind)}</td><td>${esc(e.detail ?? "")}</td><td>${esc(e.client)}</td><td>${esc(e.version)}</td>
      <td>${esc(e.plan ?? "")}</td><td>${e.country ? `${flag(e.country)} ${esc(e.country)}` : ""}</td><td class="num">${e.files || ""}</td>
      <td class="num">${e.bytes ? humanSize(e.bytes) : ""}</td></tr>`).join("")}
  </table></div></section></main>`;
  return layout(`Install ${esc(id)} — deskMonk admin`, body);
}

/** The installation's latest hardware as a terminal-style listing. */
export function hardwareHtml(ts: string, specsJson: string): string {
  let specs: Record<string, string>[] = [];
  try {
    specs = JSON.parse(specsJson);
  } catch {}
  let out = `<pre class=term><span class=dim>root@deskmonk:~$ ./hwinfo --brands  # ${esc(ts)} UTC</span>\n`;
  for (const s of specs) {
    const field = (k: string) => esc(s[k] ?? "");
    const brand = field("brand") ? `<span class=b>[${field("brand").toUpperCase()}]</span> ` : "";
    out += `<span class=k>${esc((KIND_LABEL[s.kind ?? ""] ?? s.kind ?? "").toUpperCase())}</span> ${brand}${field("name")}\n`;
    if (field("detail")) out += `<span class=dim>${" ".repeat(12)} └─ ${field("detail")}</span>\n`;
  }
  return out + "<span class=dim>root@deskmonk:~$</span> <span class=cur>█</span></pre>";
}

// ---------- small pieces ----------

export function esc(s: string): string {
  return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&#39;");
}

export function humanSize(bytes: number): string {
  const UNITS = ["B", "KB", "MB", "GB", "TB"];
  let [size, unit] = [Number(bytes) || 0, 0];
  while (size >= 1024 && unit < UNITS.length - 1) {
    size /= 1024;
    unit++;
  }
  return unit === 0 ? `${size} B` : `${size.toFixed(1)} ${UNITS[unit]}`;
}

const date = (ts: number) => new Date(ts * 1000).toISOString().slice(0, 10);
const dateTime = (ts: number) => new Date(ts * 1000).toISOString().slice(0, 16).replace("T", " ");
const usd = (n: number) => `$${Number.isInteger(n) ? n : n.toFixed(2)}`;
const count = (n: number, tone = "") => (n > 0 ? `<span class="count${tone ? ` ${tone}` : ""}">${n}</span>` : "");

function ago(ts: number): string {
  const s = now() - ts;
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min ago`;
  if (s < DAY) return `${Math.round(s / 3600)} h ago`;
  if (s < 30 * DAY) return `${Math.round(s / DAY)} days ago`;
  return date(ts);
}

function ratio(n: number, of: number): string {
  if (!of) return `<span class="muted">—</span>`;
  return `${Math.round((n / of) * 100)}% <span class="muted">(${n}/${of})</span>`;
}

function delta(a: number, b: number): string {
  const tone = a > b ? "up" : a < b ? "down" : "muted";
  return `<p class="d"><span class="${tone}">${a > b ? "▲" : a < b ? "▼" : "•"} ${Math.abs(a - b)}</span> vs ${b} yesterday</p>`;
}

const deltaText = (yesterday: string) => `<p class="d"><span class="muted">${yesterday} yesterday</span></p>`;

export function compareVersions(a: string, b: string): number {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pb[i] || 0) - (pa[i] || 0);
    if (d) return d;
  }
  return b.localeCompare(a);
}

function osName(os: string): string {
  return { macos: "macOS", windows: "Windows", linux: "Linux", ios: "iOS", android: "Android", all: "All" }[os] ?? esc(os ?? "?");
}

function osIcon(os: string): string {
  if (os === "windows") return WINDOWS_ICON;
  if (os === "linux") return "🐧";
  if (os === "macos") return icon("apple");
  if (os === "ios") return icon("phone");
  if (os === "android") return "🤖";
  return "•";
}

/** Bars per platform: always macOS, Windows, Linux and the four mobile devices, zeros included, so every tile reads the same way; others when they have a count. */
function osBars(counts: Record<string, number>, fmt: (n: number) => string = String, always = [...OSES, ...DEVICE_KEYS]): string {
  const keys = [...always, ...Object.keys(counts).filter((k) => !always.includes(k) && counts[k] > 0)];
  const total = Math.max(1, keys.reduce((a, k) => a + (counts[k] ?? 0), 0));
  return `<div class="osbars">${keys
    .map((key) => {
      const n = counts[key] ?? 0;
      const os = key.split(":")[0];
      return `<div class="osrow os-${[...OSES, ...MOBILE].includes(os) ? os : "other"}${n ? "" : " zero"}"><span class="ic">${osIcon(os)}</span><span class="nm">${platformName(key)}</span>
        <b>${fmt(n)}</b><span class="track"><i style="width:${Math.round((n / total) * 100)}%"></i></span></div>`;
    })
    .join("")}</div>`;
}

/** Bars for any small set: [label, count, color class]. */
function bars3(rows: [string, number, string][]): string {
  const total = Math.max(1, rows.reduce((a, [, n]) => a + n, 0));
  return `<div class="osbars">${rows
    .map(([label, n, cls]) => `<div class="osrow c-${cls}${n ? "" : " zero"}"><span class="ic">•</span><span class="nm">${label}</span><b>${n}</b>
      <span class="track"><i style="width:${Math.round((n / total) * 100)}%"></i></span></div>`)
    .join("")}</div>`;
}

function hbars(rows: [string, number][], fmt: (n: number) => string = humanSize): string {
  const most = Math.max(1, ...rows.map(([, n]) => n));
  return `<div class="hbars">${rows
    .map(([label, n]) => `<div class="hbar"><span>${label}</span><span class="track"><i style="width:${Math.round((n / most) * 100)}%"></i></span><b>${fmt(n)}</b></div>`)
    .join("")}</div>`;
}

function tile(t: { icon: string; tone: string; title: string; hint?: string; value: number | string; extra?: string; rows: string; foot?: string; footIcon?: string; cls?: string }): string {
  return `<div class="stat${t.cls ? ` ${t.cls}` : ""}">
    <div class="head"><span class="ico ${t.tone}">${icon(t.icon)}</span><p class="k">${t.title}${t.hint ? `<small>${t.hint}</small>` : ""}</p></div>
    <div class="v">${t.value}</div>${t.extra ?? ""}
    ${t.rows}
    ${t.foot ? `<p class="foot-line">${icon(t.footIcon ?? "bars")}<span>${t.foot}</span></p>` : ""}
  </div>`;
}

const stat = (k: string, v: string, d = "") => `<div class="stat plain"><p class="k">${k}</p><div class="v">${v}</div>${d ? `<p class="d">${d}</p>` : ""}</div>`;

function planBadge(s: Row): string {
  const manual = s.customer_id === "manual" ? ` <span class="badge manual">manual</span>` : "";
  return `<span class="badge">${s.plan === "yearly" ? "yearly" : s.plan === "monthly" ? "monthly" : "—"}</span>${manual}`;
}

function statusBadge(s: Row, t: number): string {
  if (s.status === "canceled" || s.period_end <= t) return `<span class="badge off">ended</span>`;
  if (s.status === "past_due") return `<span class="badge warn">past due</span>`;
  if (s.status === "trialing") return `<span class="badge new">trial</span>`;
  return `<span class="badge on">active</span>`;
}

/** A country code as its flag emoji (two regional indicator letters). */
function flag(cc: string): string {
  return /^[A-Z]{2}$/.test(cc) && cc !== "XX" && cc !== "T1" ? String.fromCodePoint(...[...cc].map((c) => 0x1f1a5 + c.charCodeAt(0))) : "🏳️";
}

function countryName(cc: string): string {
  if (cc === "?" || !cc) return "Unknown";
  if (cc === "T1") return "Tor";
  try {
    return new Intl.DisplayNames(["en"], { type: "region" }).of(cc) ?? cc;
  } catch {
    return cc;
  }
}

/** Today's active users, one small card per country. */
function places(countries: Row[]): string {
  const list = countries.filter((c) => c.today > 0).sort((a, b) => b.today - a.today);
  const total = list.reduce((a, c) => a + c.today, 0);
  return `<div class="places"><div class="split">${worldMap(list)}<div>
    <p class="title">${icon("globe")}Active today by country <span>(${total})</span></p>
    ${list.length ? `<div class="list">${list.map((c) => `<div class="place" title="${esc(countryName(c.country))}"><span class="flag">${flag(c.country)}</span>${esc(c.country === "?" ? "unknown" : c.country)}<b>${c.today}</b></div>`).join("")}</div>`
      : `<p class="muted" style="margin:0">Nobody has opened deskMonk yet today.</p>`}</div></div></div>`;
}

/** The land as one path of dots, built once per isolate: it never changes. */
let landPath = "";
function land(): string {
  if (landPath) return landPath;
  const parts: string[] = [];
  LAND_ROWS.forEach((hex, y) => {
    const bits = [...hex].map((c) => parseInt(c, 16).toString(2).padStart(4, "0")).join("");
    for (let x = 0; x < bits.length; x++) if (bits[x] === "1") parts.push(`M${x + 0.5} ${y + 0.5}h0`);
  });
  landPath = parts.join("");
  return landPath;
}

/** The dotted world with a glowing point on each country seen today, sized by how many.
 *  Countries without a known point ("?", Tor) are only in the list beside it. */
export function worldMap(list: Row[]): string {
  const width = LAND_ROWS[0].length * 4;
  const peak = Math.max(1, ...list.map((c) => c.today));
  const spots = list
    .map((c) => {
      const at = COUNTRY_POINTS[c.country];
      if (!at) return "";
      const x = ((at[0] + 180) / MAP_STEP).toFixed(1);
      const y = ((MAP_NORTH - at[1]) / MAP_STEP + 0.5).toFixed(1);
      const size = 0.8 + Math.sqrt(c.today / peak) * 1.1;
      return `<g class="spot"><title>${esc(countryName(c.country))}: ${c.today}</title>
        <circle class="halo" cx="${x}" cy="${y}" r="${(size * 2.6).toFixed(2)}"/><circle class="core" cx="${x}" cy="${y}" r="${size.toFixed(2)}"/></g>`;
    })
    .join("");
  return `<svg class="world" viewBox="0 0 ${width} ${LAND_ROWS.length}" role="img" aria-label="Map of today's active users">
    <path class="land" d="${land()}"/>${spots}</svg>`;
}

// ---------- page frame ----------

const ICON_PATHS: Record<string, string> = {
  grid: '<rect width="7" height="7" x="3" y="3" rx="1"/><rect width="7" height="7" x="14" y="3" rx="1"/><rect width="7" height="7" x="14" y="14" rx="1"/><rect width="7" height="7" x="3" y="14" rx="1"/>',
  chart: '<path d="M3 3v18h18"/><path d="M18 17V9"/><path d="M13 17V5"/><path d="M8 17v-3"/>',
  bars: '<path d="M6 20v-6"/><path d="M12 20V8"/><path d="M18 20V4"/>',
  userPlus: '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M19 8v6"/><path d="M22 11h-6"/>',
  users: '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>',
  cart: '<circle cx="8" cy="21" r="1"/><circle cx="19" cy="21" r="1"/><path d="M2.05 2.05h2l2.66 12.42a2 2 0 0 0 2 1.58h9.78a2 2 0 0 0 1.95-1.57l1.65-7.43H5.12"/>',
  globe: '<circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/><path d="M2 12h20"/>',
  download: '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5"/><path d="M12 15V3"/>',
  clock: '<circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/>',
  laptop: '<rect width="16" height="11" x="4" y="4" rx="2"/><path d="M2 20h20"/>',
  file: '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M16 13H8"/><path d="M16 17H8"/>',
  card: '<rect width="20" height="14" x="2" y="5" rx="2"/><path d="M2 10h20"/>',
  key: '<circle cx="7.5" cy="15.5" r="5.5"/><path d="m21 2-9.6 9.6"/><path d="m15.5 7.5 3 3L22 7l-3-3"/>',
  phone: '<rect x="7" y="2" width="10" height="20" rx="2"/><path d="M11 18h2"/>',
  wallet: '<path d="M12 2v20"/><path d="M17 5H9.5a3.5 3.5 0 0 0 0 7h5a3.5 3.5 0 0 1 0 7H6"/>',
  tools: '<path d="M4 21v-7"/><path d="M4 10V3"/><path d="M12 21v-9"/><path d="M12 8V3"/><path d="M20 21v-5"/><path d="M20 12V3"/><path d="M2 14h4"/><path d="M10 8h4"/><path d="M18 16h4"/>',
  calendar: '<rect width="18" height="18" x="3" y="4" rx="2"/><path d="M16 2v4"/><path d="M8 2v4"/><path d="M3 10h18"/>',
  star: '<path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01z"/>',
  broom: '<path d="m13 11 9-9"/><path d="M14.6 12.6c.8.8.9 2.1.2 3L10 22l-8-8 6.4-4.8c.9-.7 2.2-.6 3 .2Z"/><path d="m6.8 10.4 6.8 6.8"/><path d="m5 17 1.5 1.5"/>',
  apple: '<path d="M12 6.5V3a1 1 0 0 1 1-1"/><path d="M18.2 21A15 15 0 0 0 22 11a6 6 0 0 0-10-4.5A6 6 0 0 0 2 11a15 15 0 0 0 3.8 10 3 3 0 0 0 3.6.6 5.5 5.5 0 0 1 5.2 0 3 3 0 0 0 3.6-.6"/>',
};

const icon = (name: string) => `<svg class="i" viewBox="0 0 24 24" aria-hidden="true">${ICON_PATHS[name] ?? ""}</svg>`;
const WINDOWS_ICON = `<svg viewBox="0 0 24 24" aria-hidden="true" fill="currentColor" class="win"><path d="M3 5.5 10.5 4.4v7.1H3zM11.5 4.3 21 3v8.5h-9.5zM3 12.5h7.5v7.1L3 18.5zM11.5 12.5H21V21l-9.5-1.3z"/></svg>`;

/** Lights the section on screen in the side list, opens folded sections when linked, `/` jumps to the search. */
const SHELL_SCRIPT = `(() => {
  const links = [...document.querySelectorAll('.side a[href^="#"]')];
  const open = (id) => { const el = document.getElementById(id); if (el && el.tagName === 'DETAILS') el.open = true; };
  for (const a of links) a.addEventListener('click', () => open(a.hash.slice(1)));
  if (location.hash) open(location.hash.slice(1));
  const zones = links.map((a) => document.getElementById(a.hash.slice(1))).filter(Boolean);
  const light = () => {
    let current = zones[0];
    for (const z of zones) if (z.getBoundingClientRect().top < 140) current = z;
    if (innerHeight + scrollY >= document.body.scrollHeight - 4) current = zones[zones.length - 1];
    for (const a of links) a.classList.toggle('on', current && a.hash === '#' + current.id);
  };
  addEventListener('scroll', light, { passive: true });
  light();
  addEventListener('keydown', (e) => {
    if (e.key !== '/' || /INPUT|TEXTAREA|SELECT/.test(document.activeElement?.tagName ?? '')) return;
    e.preventDefault();
    const q = document.getElementById('q');
    q?.scrollIntoView({ block: 'center' });
    q?.focus();
  });
})();`;

function layout(title: string, body: string): string {
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="robots" content="noindex"><title>${title}</title><link rel="icon" href="/logo.svg"><style>${STYLE}</style></head><body>${body}</body></html>`;
}

const STYLE = `
:root { color-scheme: dark; --bg:#0f1117; --card:#171a23; --card-2:#1d212c; --border:#262b38; --text:#e6e8ef; --muted:#8a91a5; --faint:#5b6275;
  --brand:#7c8cff; --accent:#4ade80; --red:#f87171; --orange:#fb923c; }
* { box-sizing: border-box; }
body { margin: 0; font: 14px/1.5 Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif; color: var(--text); background: var(--bg); }
a { color: var(--brand); }
code { color: var(--brand); background: rgba(124,140,255,.1); padding: 1px 5px; border-radius: 5px; font-size: 12.5px; }
code.k { letter-spacing: .5px; user-select: all; }
.appbar { position: sticky; top: 0; z-index: 5; display: flex; align-items: center; gap: 12px; padding: 8px 24px;
  background: rgba(15,17,23,.92); backdrop-filter: blur(8px); border-bottom: 1px solid var(--border); }
.appbar .brand { display: flex; align-items: center; gap: 10px; font-size: 20px; font-weight: 700; color: var(--text); text-decoration: none; }
.appbar .brand img { width: 32px; height: 32px; border-radius: 8px; }
.appbar .tag { font-size: 11px; font-weight: 600; letter-spacing: .6px; color: var(--muted); background: var(--card-2); border-radius: 6px; padding: 4px 8px; }
.appbar .spacer { flex: 1; }
.appbar a.btn, .appbar .env { display: inline-flex; align-items: center; gap: 7px; font-size: 13px; color: var(--text); text-decoration: none;
  border: 1px solid var(--border); border-radius: 9px; padding: 5px 11px; }
.appbar a.btn:hover { border-color: var(--brand); }
.appbar .env::before { content: ""; width: 8px; height: 8px; border-radius: 50%; background: var(--orange); }
.appbar .env.prod { color: var(--accent); border-color: rgba(74,222,128,.45); }
.appbar .env.prod::before { background: var(--accent); }
.shell { display: grid; grid-template-columns: 200px minmax(0, 1fr); gap: 22px; padding: 0 24px; }
.side { position: sticky; top: 56px; align-self: start; padding: 12px 0; display: flex; flex-direction: column; gap: 4px; }
.side a { display: flex; align-items: center; gap: 10px; color: var(--muted); text-decoration: none; font-size: 14px; padding: 8px 12px;
  border-radius: 10px; border: 1px solid transparent; }
.side a:hover { color: var(--text); background: var(--card); }
.side a.on { color: var(--brand); background: rgba(124,140,255,.1); border-color: rgba(124,140,255,.25); }
.side a .count { margin-left: auto; font-size: 11px; color: var(--muted); }
.side a .count.warn { color: #0f1117; background: var(--orange); border-radius: 999px; padding: 0 7px; }
.side .hint { font-size: 11px; color: var(--faint); padding: 14px 12px 0; }
kbd { font-family: inherit; border: 1px solid var(--border); border-radius: 4px; padding: 0 5px; }
.main { padding: 12px 0 40px; min-width: 0; }
.main.solo { max-width: 1200px; margin: 0 auto; padding: 12px 24px 40px; }
h1.title { font-size: 22px; margin: 4px 0; }
.zone, .card { scroll-margin-top: 76px; }
.card { background: var(--card); border: 1px solid var(--border); border-radius: 14px; padding: 16px 20px; margin-bottom: 16px; }
.label { font-size: 12px; font-weight: 600; color: var(--muted); text-transform: uppercase; letter-spacing: .6px; margin: 0 0 10px; }
.label .aside { text-transform: none; font-weight: 400; letter-spacing: 0; color: var(--faint); }
.grid { display: grid; gap: 12px; grid-template-columns: repeat(auto-fit, minmax(210px, 1fr)); }
.grid.five { grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); }
.stat { display: flex; flex-direction: column; background: var(--card-2); border: 1px solid var(--border); border-radius: 12px; padding: 11px 14px; }
.stat .head { display: flex; gap: 12px; align-items: flex-start; }
.stat .k { margin: 0; font-size: 14px; font-weight: 600; line-height: 1.3; }
.stat.plain .k { color: var(--muted); font-weight: 500; font-size: 13px; }
.stat .k small { display: block; font-size: 12px; font-weight: 400; color: var(--muted); margin-top: 1px; }
.stat .k .info { color: var(--faint); cursor: help; font-weight: 400; }
.stat .v { font-size: 30px; font-weight: 800; line-height: 1.15; margin-top: 6px; font-variant-numeric: tabular-nums; }
.stat .d { font-size: 12.5px; color: var(--muted); margin: 4px 0 0; }
.stat.alert { box-shadow: inset 3px 0 0 var(--orange); }
.stat.alert .v { color: var(--orange); }
.up { color: var(--accent); } .down { color: var(--red); } .muted { color: var(--muted); }
.ico { display: inline-flex; align-items: center; justify-content: center; width: 34px; height: 34px; border-radius: 10px; flex: none; }
svg.i { width: 18px; height: 18px; flex: none; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; vertical-align: -4px; }
svg.win { width: 13px; height: 13px; vertical-align: -2px; color: #3b9cff; }
.t-blue { color: #60a5fa; background: rgba(96,165,250,.14); } .t-red { color: #f87171; background: rgba(248,113,113,.14); }
.t-green { color: #4ade80; background: rgba(74,222,128,.13); } .t-teal { color: #2dd4bf; background: rgba(45,212,191,.13); }
.t-violet { color: #a78bfa; background: rgba(167,139,250,.15); } .t-orange { color: #fb923c; background: rgba(251,146,60,.14); }
.osbars { border-top: 1px solid var(--border); margin-top: 8px; padding-top: 7px; display: grid; gap: 3px; }
.osrow { display: grid; grid-template-columns: 18px 88px minmax(26px, auto) 1fr; align-items: center; gap: 8px; font-size: 12.5px; line-height: 1.35; }
.osrow .nm { color: var(--muted); } .osrow b { font-variant-numeric: tabular-nums; } .osrow.zero b { color: var(--faint); font-weight: 500; }
.osrow .ic { display: inline-flex; justify-content: center; font-size: 14px; line-height: 1; }
.osrow .ic svg { width: 15px; height: 15px; }
.track { display: block; height: 6px; background: var(--bg); border-radius: 99px; overflow: hidden; }
.track i { display: block; height: 100%; border-radius: 99px; background: var(--brand); }
.os-windows .track i { background: #3b9cff; } .os-linux .track i { background: #f59e0b; } .os-macos .track i { background: #a855f7; }
.os-macos .ic { color: #e6e8ef; } .os-other .track i { background: var(--muted); }
.os-ios .ic { color: #e6e8ef; } .os-ios .track i { background: #ec4899; } .os-android .track i { background: #22c55e; }
.c-play .ic { color: #22c55e; } .c-play .track i { background: #22c55e; }
.c-web .track i, .c-web .ic { background: #60a5fa; color: #60a5fa; } .c-web .track i { background: #60a5fa; }
.c-store .ic { color: #a855f7; } .c-store .track i { background: #a855f7; }
.c-manual .ic { color: var(--orange); } .c-manual .track i { background: var(--orange); }
.c-web .ic { background: none; }
.stat .foot-line { display: flex; gap: 8px; align-items: flex-start; margin: auto 0 0; padding-top: 7px; border-top: 1px solid var(--border);
  margin-top: 8px; font-size: 12px; line-height: 1.4; color: var(--muted); }
.stat .foot-line svg.i { width: 15px; height: 15px; margin-top: 1px; }
.bars { display: flex; align-items: flex-end; gap: 4px; height: 80px; margin-top: 12px; }
.bars div { flex: 1; background: var(--brand); opacity: .7; border-radius: 3px 3px 0 0; min-height: 2px; }
.bars div:last-child { opacity: 1; }
.stack { display: flex; align-items: stretch; gap: 3px; height: 90px; margin-top: 12px; }
.stack > span { flex: 1; display: flex; flex-direction: column; justify-content: flex-end; gap: 1px; min-width: 0; border-bottom: 2px solid var(--border); }
.stack i, .legend i { display: block; border-radius: 2px; } .stack i.ios-Phone, .legend i.ios-Phone { background: #ec4899; } .stack i.ios-Tablet, .legend i.ios-Tablet { background: #f9a8d4; }
.stack i.android-Phone, .legend i.android-Phone { background: #22c55e; } .stack i.android-Tablet, .legend i.android-Tablet { background: #86efac; } .stack i.ios-, .stack i.android- { background: var(--muted); }
.stack > span:hover { background: var(--card-2); }
.legend { display: inline-flex; gap: 12px; margin-left: 10px; text-transform: none; letter-spacing: 0; font-weight: 400; }
.legend i { display: inline-block; width: 9px; height: 9px; margin-right: 5px; }
.two { display: grid; gap: 20px; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); margin-top: 22px; }
.two .label { margin-bottom: 8px; }
.dot { display: inline-block; width: 8px; height: 8px; border-radius: 50%; margin-right: 7px; background: var(--faint); }
.dot.live { background: var(--accent); animation: pulse 1.6s ease-out infinite; } .dot.recent { background: #2dd4bf; }
.dot.dormant { background: var(--orange); } .dot.gone { background: var(--red); }
.stat.live { border-color: rgba(74,222,128,.45); } .stat.live .v { color: var(--accent); }
@keyframes pulse { 0% { box-shadow: 0 0 0 0 rgba(74,222,128,.6); } 100% { box-shadow: 0 0 0 8px rgba(74,222,128,0); } }
@media (prefers-reduced-motion: reduce) { .dot.live { animation: none; } }
.bars-axis { display: flex; justify-content: space-between; font-size: 11px; color: var(--faint); margin-top: 4px; }
.hbars { display: grid; gap: 8px; }
.hbar { display: grid; grid-template-columns: 130px 1fr 90px; gap: 12px; align-items: center; }
.hbar .track { height: 10px; } .hbar b { text-align: right; font-variant-numeric: tabular-nums; }
.places { margin-top: 12px; background: var(--card-2); border: 1px solid var(--border); border-radius: 12px; padding: 10px 14px; }
.places .title { display: flex; align-items: center; gap: 10px; font-size: 14px; font-weight: 600; margin: 0 0 8px; }
.places .title svg { color: var(--brand); } .places .title span { font-weight: 400; color: var(--muted); }
.places .list { display: grid; gap: 8px; grid-template-columns: repeat(auto-fill, minmax(110px, 1fr)); }
.place { display: flex; align-items: center; gap: 8px; background: var(--bg); border: 1px solid var(--border); border-radius: 9px; padding: 5px 10px; font-size: 13px; }
.place .flag { font-size: 17px; line-height: 1; } .place b { margin-left: auto; font-variant-numeric: tabular-nums; }
.places .split { display: grid; gap: 18px; align-items: center; grid-template-columns: auto minmax(0, 1fr); }
svg.world { height: 128px; width: auto; max-width: 100%; display: block; }
svg.world .land { fill: none; stroke: #2f3550; stroke-width: .62; stroke-linecap: round; }
svg.world .halo { fill: var(--brand); opacity: .22; animation: halo 2.6s ease-in-out infinite; transform-box: fill-box; transform-origin: center; }
svg.world .core { fill: #c3caff; filter: drop-shadow(0 0 1.2px var(--brand)); }
svg.world .spot:hover .core { fill: #fff; }
@keyframes halo { 0%, 100% { transform: scale(.8); opacity: .3; } 50% { transform: scale(1.15); opacity: .12; } }
@media (prefers-reduced-motion: reduce) { svg.world .halo { animation: none; } }
.scroll { overflow-x: auto; }
table { border-collapse: collapse; width: 100%; font-size: 13px; }
th, td { border-bottom: 1px solid var(--border); padding: 8px 10px; text-align: left; white-space: nowrap; }
th { color: var(--muted); font-size: 11.5px; font-weight: 600; }
td.num, th.num { text-align: right; font-variant-numeric: tabular-nums; }
tr:last-child td { border-bottom: 0; }
table tr:hover td { background: rgba(124,140,255,.05); }
tr.total td { border-top: 1px solid var(--border); font-weight: 600; }
tr.mine td { opacity: .6; }
.foot { font-size: 12.5px; margin: 10px 0 0; }
details.card { padding: 0; }
details.card > summary { cursor: pointer; padding: 15px 20px; font-weight: 600; list-style: none; }
details.card > summary::-webkit-details-marker { display: none; }
details.card > summary::before { content: "▸ "; color: var(--muted); }
details.card[open] > summary::before { content: "▾ "; }
details.card > .body { padding: 0 20px 18px; }
details.card.warnbox > summary { color: var(--orange); }
details.act summary { cursor: pointer; color: var(--brand); list-style: none; font-size: 12.5px; }
details.act summary::-webkit-details-marker { display: none; }
.acts { display: flex; flex-wrap: wrap; gap: 6px; padding-top: 8px; }
.badge { display: inline-block; font-size: 11px; border-radius: 999px; padding: 2px 8px; background: var(--card-2); color: var(--muted); white-space: nowrap; }
.badge.on { background: rgba(74,222,128,.15); color: var(--accent); } .badge.off { color: var(--faint); }
.badge.new { background: rgba(124,140,255,.18); color: var(--brand); } .badge.warn { background: rgba(251,146,60,.15); color: var(--orange); }
.badge.you, .badge.manual { background: rgba(251,146,60,.15); color: var(--orange); }
.badge.store { background: rgba(168,85,247,.16); color: #c084fc; }
.group { border-left: 3px solid var(--border); padding-left: 14px; margin-top: 20px; }
.group.paddle { border-left-color: var(--brand); } .group.manual { border-left-color: var(--orange); }
.group-head { margin: 0 0 8px; display: flex; gap: 8px; align-items: center; }
.row { display: flex; flex-wrap: wrap; gap: 12px; align-items: flex-end; }
label.field { display: block; color: var(--muted); font-size: 12px; margin-bottom: 4px; }
input, select { font: inherit; background: var(--bg); color: var(--text); border: 1px solid var(--border); border-radius: 8px; padding: 9px 12px; width: 100%; }
input:focus, select:focus { outline: none; border-color: var(--brand); }
form.inline { display: inline-flex; gap: 6px; align-items: center; margin: 0; }
form.inline input { width: 72px; padding: 5px 8px; font-size: 12.5px; }
button { font: inherit; font-weight: 600; font-size: 13px; color: #0f1117; background: var(--brand); border: 0; border-radius: 8px; padding: 9px 14px; cursor: pointer; white-space: nowrap; }
button:hover { filter: brightness(1.1); }
button.quiet { background: transparent; border: 1px solid var(--border); color: var(--text); }
button.danger { background: var(--red); }
form.inline button { font-size: 12px; padding: 5px 10px; }
.btn-link { align-self: center; }
.note { border-radius: 10px; padding: 10px 14px; margin: 12px 0; }
.note.ok { background: rgba(74,222,128,.1); color: var(--accent); } .note.err { background: rgba(248,113,113,.12); color: var(--red); }
.term { background: #070b08; color: #39ff14; border: 1px solid #1e5c2b; border-radius: 12px; padding: 16px 18px; overflow-x: auto; margin: 0 0 16px;
  font: 13px/1.6 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; text-shadow: 0 0 6px rgba(57,255,20,.35); }
.term .dim { color: #1f9e45; } .term .k { color: #1f9e45; display: inline-block; min-width: 12ch; } .term .b { font-weight: 700; }
.term .cur { animation: blink 1s steps(1) infinite; } @keyframes blink { 50% { opacity: 0; } }
@media (min-width: 901px) and (max-width: 1299px) {
  .shell { grid-template-columns: 52px minmax(0, 1fr); gap: 16px; }
  .side a { justify-content: center; padding: 10px 0; position: relative; }
  .side a .nav-label, .side .hint { display: none; }
  .side a .count { position: absolute; top: 2px; right: 2px; margin: 0; font-size: 10px; }
}
@media (max-width: 900px) {
  .shell { grid-template-columns: minmax(0, 1fr); padding: 0 16px; }
  .side { flex-direction: row; overflow-x: auto; top: 50px; z-index: 4; background: var(--bg); padding: 8px 0; }
  .side a { white-space: nowrap; padding: 7px 10px; } .side .hint { display: none; }
  .appbar { padding: 8px 16px; } .appbar a.btn span { display: none; }
  .main.solo { padding: 12px 16px 40px; }
  .hbar { grid-template-columns: 100px 1fr 70px; }
  .places .split { grid-template-columns: minmax(0, 1fr); }
  svg.world { width: 100%; height: auto; }
}
`;
