//! Outgoing mail through Resend (https://resend.com). Without RESEND_API_KEY nothing is sent and every caller carries on.
//! POST /api/contact {message, email?, install_id, version?, os?}  the app's contact form → OWNER_EMAIL (kept in D1 too)
//! Monday 08:00 UTC (cron in wrangler.toml)                     weekly summary of installs and sales → OWNER_EMAIL

export interface MailEnv {
  DB: D1Database;
  RESEND_API_KEY?: string;
  /** Sender on the domain verified in Resend, e.g. "deskMonk <support@deskmonk.app>". */
  MAIL_FROM?: string;
  /** Where contact messages and the weekly summary go. */
  OWNER_EMAIL?: string;
}

const EMAIL = /^[^\s@<>]{1,64}@[^\s@<>]{1,190}\.[^\s@<>]{2,}$/;
const INSTALL_ID = /^[A-Za-z0-9._-]{1,64}$/;
const MAX_MESSAGE = 5000;
/** Contact messages accepted per hour: from one installation, and from everyone together (protects the Resend quota). */
const PER_INSTALL = 3;
const PER_HOUR = 30;

/** Sends one plain-text mail; false when mail isn't set up or Resend refused it (logged). Never throws. */
export async function sendMail(env: MailEnv, m: { to: string; subject: string; text: string; replyTo?: string }): Promise<boolean> {
  if (!env.RESEND_API_KEY || !env.MAIL_FROM) return false;
  try {
    const res = await fetch("https://api.resend.com/emails", {
      method: "POST",
      headers: { Authorization: `Bearer ${env.RESEND_API_KEY}`, "Content-Type": "application/json" },
      body: JSON.stringify({ from: env.MAIL_FROM, to: [m.to], subject: m.subject, text: m.text, ...(m.replyTo ? { reply_to: m.replyTo } : {}) }),
    });
    if (!res.ok) console.log(`resend: ${res.status} ${(await res.text()).slice(0, 300)}`);
    return res.ok;
  } catch (e) {
    console.log(`resend: ${e}`);
    return false;
  }
}

/** The mail a buyer gets with their key, once, right after the purchase. */
// shortcut: English only (the buyer's site language isn't passed to Paddle); add a lang to custom_data if buyers ask.
export function keyMail(key: string, plan: string): { subject: string; text: string } {
  return {
    subject: "Your deskMonk Pro license key",
    text: `Thank you for buying deskMonk Pro (${plan}).

Your license key: ${key}

To activate: open deskMonk, paste the key into "Have a license key?" at the top of the window and click Activate. It works on up to 3 computers.
If you bought from inside deskMonk, that computer has already turned Pro by itself.
In the terminal version, press p and type the key.

See your plan, or change or cancel the subscription: https://deskmonk.app/account
Questions: just reply to this email.

deskMonk
https://deskmonk.app
`,
  };
}

/** Returns a response for /api/contact, or null for any other path. */
export async function handleMail(req: Request, env: MailEnv, path: string): Promise<Response | null> {
  if (path !== "/api/contact") return null;
  if (req.method !== "POST") return new Response(null, { status: 405 });
  const body = await req.text();
  if (body.length > MAX_MESSAGE * 2) return json({ error: "bad_request" }, 400);
  let v: any;
  try {
    v = JSON.parse(body);
  } catch {
    return json({ error: "bad_request" }, 400);
  }
  const message = typeof v?.message === "string" ? v.message.trim() : "";
  const email = typeof v?.email === "string" ? v.email.trim() : "";
  const install = typeof v?.install_id === "string" && INSTALL_ID.test(v.install_id) ? v.install_id : null;
  const short = (s: unknown) => (typeof s === "string" ? s.slice(0, 40) : "");
  if (message === "" || message.length > MAX_MESSAGE || install === null) return json({ error: "bad_request" }, 400);
  if (email !== "" && !EMAIL.test(email)) return json({ error: "bad_email" }, 400);

  const now = Math.floor(Date.now() / 1000);
  const counts = await env.DB.prepare("SELECT COUNT(*) AS all_, SUM(install_id = ?1) AS mine FROM messages WHERE ts > ?2")
    .bind(install, now - 3600)
    .first<{ all_: number; mine: number | null }>();
  if ((counts?.mine ?? 0) >= PER_INSTALL || (counts?.all_ ?? 0) >= PER_HOUR) return json({ error: "too_many" }, 429);

  const country = (req as { cf?: { country?: unknown } }).cf?.country;
  const where = typeof country === "string" ? country : "?";
  const version = short(v?.version), os = short(v?.os);
  // Kept first, so nothing is lost when mail isn't set up or Resend is down.
  await env.DB.prepare("INSERT INTO messages(ts, install_id, email, message, version, os, country) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")
    .bind(now, install, email || null, message, version, os, where)
    .run();
  if (env.OWNER_EMAIL) {
    await sendMail(env, {
      to: env.OWNER_EMAIL,
      subject: `deskMonk message from ${email || "an anonymous user"}`,
      text: `${message}\n\n--\nFrom: ${email || "(no email given)"}\ndeskMonk ${version} on ${os}, country ${where}\nInstallation: ${install}\n`,
      replyTo: email || undefined,
    });
  }
  return json({ ok: true });
}

/** Numbers for the weekly mail: the 7 days before `now` (unix seconds). Installs marked as the operator's own don't count. */
export async function weekNumbers(db: D1Database, now: number) {
  const since = now - 7 * 86400;
  const real = "install_id NOT IN (SELECT install_id FROM ignored_installs)";
  const one = async <T>(sql: string, ...args: unknown[]) => (await db.prepare(sql).bind(...args).first<T>())!;
  const installs = await one<{ fresh: number; active: number; opens: number }>(
    `SELECT COUNT(DISTINCT CASE WHEN first >= ?1 THEN install_id END) AS fresh, COUNT(DISTINCT CASE WHEN last >= ?1 THEN install_id END) AS active,
       COALESCE(SUM(opens), 0) AS opens
     FROM (SELECT install_id, MIN(ts) AS first, MAX(ts) AS last, SUM(kind = 'open' AND ts >= ?1) AS opens FROM events WHERE ${real} GROUP BY install_id)`,
    since,
  );
  const os = (await db
    .prepare(`SELECT os, COUNT(DISTINCT install_id) AS n FROM events WHERE ts >= ?1 AND ${real} GROUP BY os ORDER BY n DESC`)
    .bind(since)
    .all<{ os: string; n: number }>()).results;
  const subs = await one<{ fresh: number; monthly: number; yearly: number; canceled: number }>(
    `SELECT SUM(created_at >= ?1 AND customer_id NOT IN ('manual', 'pelamx')) AS fresh,
       SUM(status IN ('active', 'trialing', 'past_due') AND plan = 'monthly' AND customer_id != 'manual') AS monthly,
       SUM(status IN ('active', 'trialing', 'past_due') AND plan = 'yearly' AND customer_id != 'manual') AS yearly,
       SUM(status = 'canceled' AND period_end >= ?1) AS canceled
     FROM subscriptions`,
    since,
  );
  const messages = await one<{ n: number }>("SELECT COUNT(*) AS n FROM messages WHERE ts >= ?1", since);
  return {
    newInstalls: installs.fresh ?? 0,
    activeInstalls: installs.active ?? 0,
    opens: installs.opens ?? 0,
    os,
    newSubs: subs.fresh ?? 0,
    monthly: subs.monthly ?? 0,
    yearly: subs.yearly ?? 0,
    canceled: subs.canceled ?? 0,
    messages: messages.n ?? 0,
  };
}

export function weeklyText(n: Awaited<ReturnType<typeof weekNumbers>>): string {
  const mrr = n.monthly * 2.5 + (n.yearly * 25) / 12;
  return `deskMonk, the last 7 days

Installs
  New: ${n.newInstalls}
  Active: ${n.activeInstalls}
  App starts: ${n.opens}
${n.os.map((o) => `  ${o.os}: ${o.n}`).join("\n") || "  (none)"}

Pro
  New subscriptions: ${n.newSubs}
  Active now: ${n.monthly} monthly, ${n.yearly} yearly (about $${mrr.toFixed(2)} a month before Paddle's fee)
  Canceled: ${n.canceled}

Messages from the app: ${n.messages}

Details: https://deskmonk.app/admin
`;
}

/** The cron job: one summary mail to the owner. */
export async function weekly(env: MailEnv): Promise<void> {
  if (!env.OWNER_EMAIL) return;
  const n = await weekNumbers(env.DB, Math.floor(Date.now() / 1000));
  await sendMail(env, { to: env.OWNER_EMAIL, subject: `deskMonk weekly: ${n.newInstalls} new installs, ${n.newSubs} new Pro`, text: weeklyText(n) });
}

function json(v: unknown, status = 200): Response {
  return new Response(JSON.stringify(v), { status, headers: { "Content-Type": "application/json" } });
}
