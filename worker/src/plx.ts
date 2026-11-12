//! plx: deskMonk Pro paid in USDC on Solana (Solana Pay). Crypto can't be charged again by itself, so each payment
//! buys one period ahead; paying again (from the same computer, or "Renew" on /account) extends the same key.
//! POST /plx {cycle, email?, install?, key?}  makes an invoice, → 303 /plx?ref=<ref>
//! GET  /plx?ref=<ref>                        QR code and wallet link; waits for the payment, then /welcome shows the key
//! A payment is looked for when /api/order/<ref> is polled and by the every-minute cron (the buyer may close the page).
//! Money goes straight to PLX_WALLET; this server holds no private key.

import { esc } from "./admin";
import { USD } from "./i18n";
import { keyMail, sendMail } from "./mail";
import { page, t } from "./pages";
import type { Lang } from "./i18n";
import { newKey, type LicenseEnv } from "./license";

/** USDC on Solana mainnet. Checking the mint stops look-alike tokens from counting as a payment. */
export const USDC_MINT = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
// shortcut: the public RPC is rate limited; set the PLX_RPC secret to a Helius/QuickNode URL once there are real buyers.
const PUBLIC_RPC = "https://api.mainnet-beta.solana.com";
const BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const ADDRESS = /^[1-9A-HJ-NP-Za-km-z]{32,44}$/;
const REF = /^[0-9a-f]{32}$/;
const DAYS = { monthly: 30, yearly: 365 } as const;
/** Payments are looked for this long after the invoice was made; later ones are sorted out by hand. */
const WATCH = 2 * 3600;

export function plxConfigured(env: LicenseEnv): boolean {
  return ADDRESS.test(env.PLX_WALLET ?? "");
}

export function base58(bytes: Uint8Array): string {
  let n = bytes.reduce((acc, b) => acc * 256n + BigInt(b), 0n);
  let out = "";
  while (n > 0n) {
    out = BASE58[Number(n % 58n)] + out;
    n /= 58n;
  }
  for (const b of bytes) {
    if (b !== 0) break;
    out = "1" + out;
  }
  return out;
}

/** USDC base units (6 decimals) of a price in dollars. */
export const usdcUnits = (usd: number): number => Math.round(usd * 1_000_000);

/** The Solana Pay link a wallet opens (or scans as a QR code). */
export function payUrl(wallet: string, units: number, reference: string, plan: string): string {
  const amount = (units / 1_000_000).toString();
  const q = new URLSearchParams({ amount, "spl-token": USDC_MINT, reference, label: "deskMonk", message: `deskMonk Pro ${plan}` });
  return `solana:${wallet}?${q.toString().replace(/\+/g, "%20")}`;
}

/** USDC base units `wallet` received in a finalized transaction (getTransaction, jsonParsed); 0 if it failed. */
export function received(tx: any, wallet: string): bigint {
  if (!tx?.meta || tx.meta.err !== null) return 0n;
  const mine = (list: unknown) =>
    new Map<number, bigint>(
      (Array.isArray(list) ? list : [])
        .filter((b: any) => b?.mint === USDC_MINT && b?.owner === wallet && typeof b?.uiTokenAmount?.amount === "string")
        .map((b: any) => [b.accountIndex, BigInt(b.uiTokenAmount.amount)]),
    );
  const pre = mine(tx.meta.preTokenBalances), post = mine(tx.meta.postTokenBalances);
  let sum = 0n;
  for (const [i, v] of post) sum += v - (pre.get(i) ?? 0n);
  return sum;
}

async function rpc(env: LicenseEnv, method: string, params: unknown[]): Promise<any> {
  const res = await fetch(env.PLX_RPC || PUBLIC_RPC, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
  });
  const v = (await res.json()) as any;
  if (!res.ok || v?.error) throw new Error(`${method}: ${res.status} ${v?.error?.message ?? ""}`);
  return v.result;
}

interface Invoice {
  ref: string;
  reference: string;
  plan: "monthly" | "yearly";
  amount: number;
  install_id: string | null;
  email: string | null;
  license_key: string | null;
  created_at: number;
  signature: string | null;
}

const now = () => Math.floor(Date.now() / 1000);

/** POST /plx: a new invoice for the plan, then its payment page. */
export async function createInvoice(req: Request, env: LicenseEnv, keyPattern: RegExp, installPattern: RegExp): Promise<Response> {
  if (!plxConfigured(env)) return new Response("plx is not set up", { status: 503 });
  let form: FormData;
  try {
    form = await req.formData();
  } catch {
    return new Response("bad request", { status: 400 });
  }
  const field = (k: string) => String(form.get(k) ?? "").trim();
  const plan = field("cycle") === "yearly" ? "yearly" : "monthly";
  const install = field("install");
  const key = field("key").toUpperCase();
  const email = field("email");
  if ((install !== "" && !installPattern.test(install)) || (key !== "" && !keyPattern.test(key)) || email.length > 200 || (email !== "" && !/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email))) {
    return new Response("bad request", { status: 400 });
  }
  const ref = [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, "0")).join("");
  const reference = base58(crypto.getRandomValues(new Uint8Array(32)));
  // shortcut: no per-visitor limit on invoices; add one if someone floods /plx (the cron only checks the newest 25).
  await env.DB.prepare(
    `INSERT INTO plx_invoices(ref, reference, plan, amount, install_id, email, license_key, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)`,
  )
    .bind(ref, reference, plan, usdcUnits(USD[plan]), install || null, email || null, key || null, now())
    .run();
  const lang = new URL(req.url).searchParams.get("lang");
  return Response.redirect(new URL(`/plx?ref=${ref}${lang ? `&lang=${encodeURIComponent(lang)}` : ""}`, req.url).toString(), 303);
}

/** GET /plx?ref=: the QR code and wallet link of an invoice. */
export async function invoicePage(req: Request, env: LicenseEnv, lang: Lang): Promise<Response> {
  const ref = new URL(req.url).searchParams.get("ref") ?? "";
  const inv = REF.test(ref) ? await env.DB.prepare("SELECT * FROM plx_invoices WHERE ref = ?1").bind(ref).first<Invoice>() : null;
  if (inv === null || !plxConfigured(env)) return Response.redirect(new URL(`/buy?lang=${lang}`, req.url).toString(), 302);
  if (inv.signature !== null) return Response.redirect(new URL(`/welcome?lang=${lang}&ref=${ref}`, req.url).toString(), 302);
  const s = t(lang);
  const url = payUrl(env.PLX_WALLET!, inv.amount, inv.reference, inv.plan);
  const amount = `${(inv.amount / 1_000_000).toFixed(2)} USDC`;
  return page(
    req,
    lang,
    `${s.plxTitle} — ${s.proTitle}`,
    `<h1>${s.plxTitle}</h1>
  <section class="paycard" style="text-align:center">
    <p style="margin-top:18px">${esc(s.planName[inv.plan] ?? inv.plan)} · <b style="color:var(--text)">${amount}</b> · Solana</p>
    <div id="qr" style="display:inline-block;background:#fff;padding:12px;border-radius:12px" role="img" aria-label="${esc(s.plxScan)}"></div>
    <p class="muted" style="margin-top:10px">${s.plxScan}</p>
    <a href="${esc(url)}"><button type="button">${s.plxOpen}</button></a>
    <p id="status" class="warn" style="margin-top:16px" aria-live="polite">${s.plxWaiting}</p>
  </section>
  <p class="muted fineprint">${s.plxFine}</p>
  <script src="https://cdnjs.cloudflare.com/ajax/libs/qrcodejs/1.0.0/qrcode.min.js"></script>
  <script>
  const c = ${JSON.stringify({ url, ref, welcome: `/welcome?lang=${lang}&ref=${ref}` }).replace(/</g, "\\u003c")};
  try { new QRCode(document.getElementById('qr'), { text: c.url, width: 220, height: 220, correctLevel: QRCode.CorrectLevel.M }); } catch (e) { console.error(e); }
  async function poll() {
    try { const o = await fetch('/api/order/' + c.ref).then((r) => r.json()); if (o.status === 'paid') { location.href = c.welcome; return; } } catch {}
    setTimeout(poll, 4000);
  }
  poll();
  </script>`,
  );
}

/** Looks on the chain for the invoice's payment (at most every 5 s per invoice) and, once found, turns it into Pro.
 *  Returns the invoice as it now is, or null if there is none with this ref. */
export async function checkInvoice(env: LicenseEnv, ref: string): Promise<Invoice | null> {
  const inv = await env.DB.prepare("SELECT * FROM plx_invoices WHERE ref = ?1").bind(ref).first<Invoice>();
  if (inv === null || inv.signature !== null || !plxConfigured(env) || now() - inv.created_at > WATCH) return inv;
  // One checker at a time per invoice: the page's polls and the cron would otherwise ask the RPC over and over.
  const claim = await env.DB.prepare("UPDATE plx_invoices SET checked_at = ?1 WHERE ref = ?2 AND checked_at <= ?3").bind(now(), ref, now() - 5).run();
  if (!claim.meta?.changes) return inv;
  try {
    const sigs = (await rpc(env, "getSignaturesForAddress", [inv.reference, { commitment: "finalized", limit: 20 }])) as any[];
    for (const s of sigs ?? []) {
      if (s?.err !== null || typeof s?.signature !== "string") continue;
      // Version 1 transactions are on mainnet now; asking for 0 only makes the RPC refuse them.
      const tx = await rpc(env, "getTransaction", [s.signature, { commitment: "finalized", encoding: "jsonParsed", maxSupportedTransactionVersion: 1 }])
        .catch((e) => (console.error("plx tx", s.signature, e), null)); // one unreadable transaction doesn't hide the others
      if (received(tx, env.PLX_WALLET!) >= BigInt(inv.amount)) return await markPaid(env, inv, s.signature);
    }
  } catch (e) {
    console.error("plx check", ref, e); // the next poll or the cron tries again
  }
  return inv;
}

/** The cron's sweep: open invoices of the last two hours, newest first. */
export async function sweepInvoices(env: LicenseEnv): Promise<void> {
  if (!plxConfigured(env)) return;
  const open = await env.DB.prepare("SELECT ref FROM plx_invoices WHERE signature IS NULL AND created_at >= ?1 ORDER BY created_at DESC LIMIT 25")
    .bind(now() - WATCH)
    .raw<[string]>();
  for (const [ref] of open) await checkInvoice(env, ref);
}

async function markPaid(env: LicenseEnv, inv: Invoice, signature: string): Promise<Invoice> {
  const db = env.DB;
  const days = DAYS[inv.plan] * 86400;
  // Renewal: the key given on /account, else this computer's earlier plx license. Only plx ones: Paddle's next
  // webhook would set its own date again.
  type Sub = { id: string; license_key: string };
  const existing = inv.license_key
    ? await db.prepare("SELECT id, license_key FROM subscriptions WHERE license_key = ?1 AND customer_id = 'plx'").bind(inv.license_key).first<Sub>()
    : inv.install_id
      ? await db.prepare("SELECT id, license_key FROM subscriptions WHERE install_id = ?1 AND customer_id = 'plx' ORDER BY period_end DESC LIMIT 1").bind(inv.install_id).first<Sub>()
      : null;
  const key = existing?.license_key ?? newKey();
  const at = now(), iso = new Date().toISOString();
  // One D1 batch is one transaction: the payment is never recorded without its key. The license statements apply only
  // while this transaction is the invoice's payment, so a repeated check never extends twice.
  // signature is UNIQUE: one transaction carrying two invoices' references pays only one of them (the batch fails).
  const paidHere = (ref: string, sig: string) => `EXISTS (SELECT 1 FROM plx_invoices WHERE ref = ${ref} AND signature = ${sig})`;
  const stmts = [
    db.prepare("UPDATE plx_invoices SET signature = ?1, paid_at = ?2, license_key = ?3 WHERE ref = ?4 AND signature IS NULL").bind(signature, at, key, inv.ref),
    existing
      ? db.prepare(`UPDATE subscriptions SET period_end = MAX(period_end, ?1) + ?2, status = 'active', plan = ?3, txn = ?4, event_at = ?5
                    WHERE id = ?6 AND txn IS NOT ?4 AND ${paidHere("?7", "?4")}`)
          .bind(at, days, inv.plan, signature, iso, existing.id, inv.ref)
      : db.prepare(`INSERT OR IGNORE INTO subscriptions(id, customer_id, status, period_end, license_key, install_id, txn, event_at, plan, created_at, email, ref)
                    SELECT ?1, 'plx', 'active', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10 WHERE ${paidHere("?10", "?5")}`)
          .bind(`plx_${inv.ref}`, at + days, key, inv.install_id, signature, iso, inv.plan, at, inv.email, inv.ref),
  ];
  if (inv.install_id !== null) {
    stmts.push(
      db.prepare(`INSERT OR IGNORE INTO activations(license_key, install_id, ts) SELECT ?1, ?2, ?3 WHERE ${paidHere("?4", "?5")}`)
        .bind(key, inv.install_id, at, inv.ref, signature),
    );
  }
  try {
    await db.batch(stmts);
  } catch (e) {
    console.error("plx paid", inv.ref, e); // nothing was written; the next poll or the cron tries again
    return inv;
  }
  const row = (await db.prepare("SELECT * FROM plx_invoices WHERE ref = ?1").bind(inv.ref).first<Invoice>())!;
  if (row.signature === signature && existing === null && inv.email !== null) {
    await sendMail(env, { to: inv.email, ...keyMail(key, inv.plan), replyTo: env.OWNER_EMAIL });
  }
  return row;
}
