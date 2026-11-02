//! deskMonk on Cloudflare Workers and D1, at deskmonk.app. The website (../docs) is served as static assets;
//! "/" and "/pricing" pass through site.ts first (translated into the visitor's language), and
//! every path without a file there comes here:
//! POST /api/events        ← apps send JSON events here
//! /admin…                 the operator's panel (admin.ts)
//! Pro licenses (Paddle):  license.ts

import { handleAdmin } from "./admin";
import { handleLicense, type LicenseEnv } from "./license";
import { handleMail, weekly } from "./mail";
import { countryOf } from "./i18n";
import { handleSite } from "./site";

export interface Env extends LicenseEnv {
  DB: D1Database;
  ADMIN_TOKEN: string;
  ASSETS: Fetcher;
}

// Room for an "open" event's hardware list.
const MAX_BODY = 16384;
const MAX_SPECS = 40;
const MAX_SPEC_TEXT = 200;
const SIZE_FIELDS = ["bytes", "duplicate_bytes", "junk_bytes", "old_download_bytes", "large_bytes"] as const;
// "ping": the phone apps' heartbeat while on screen, for the admin panel's "live now". "applock": an app was locked (phones).
const KINDS = ["scan", "clean", "open", "vault", "focus", "pro", "bought", "ping", "applock"];

export default {
  async fetch(req: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const path = new URL(req.url).pathname;
    const site = await handleSite(req, env.ASSETS, path, (kind, source, platform) =>
      // After the response, and a failed write never breaks the page.
      ctx.waitUntil(
        env.DB.prepare("INSERT INTO visits(ts, kind, source, country, platform) VALUES (?1, ?2, ?3, ?4, ?5)")
          .bind(Math.floor(Date.now() / 1000), kind, source, countryOf(req), platform)
          .run()
          .catch(() => {}),
      ),
    );
    if (site !== null) return site;

    if (req.method === "POST" && path === "/api/events") {
      const body = await req.text();
      if (body.length > MAX_BODY) return new Response("body too large", { status: 400 });
      let ev: Event;
      try {
        ev = parseEvent(body);
      } catch (e) {
        return new Response(String((e as Error).message), { status: 400 });
      }
      // Cloudflare's country guess for the request; the IP address itself is not kept.
      const country = (req as { cf?: { country?: unknown } }).cf?.country;
      await insert(env.DB, ev, typeof country === "string" && /^[A-Z0-9]{2}$/.test(country) ? country : null);
      return new Response(null, { status: 204 });
    }

    const license = await handleLicense(req, env, path);
    if (license !== null) return license;
    const mail = await handleMail(req, env, path);
    if (mail !== null) return mail;

    return (await handleAdmin(req, env, path)) ?? new Response(null, { status: 404 });
  },

  async scheduled(_event: ScheduledController, env: Env): Promise<void> {
    await weekly(env);
  },
} satisfies ExportedHandler<Env>;

export interface Event {
  install_id: string;
  version: string;
  os: string;
  client: string;
  kind: string;
  files: number;
  sizes: number[];
  /** Latest hardware list (re-serialized validated JSON), sent with "open" events. */
  specs: string | null;
  plan: string | null;
  channel: string | null;
  detail: string | null;
}

/** Validates untrusted input from the apps. Throws with the reason. */
export function parseEvent(body: string): Event {
  let v: any;
  try {
    v = JSON.parse(body);
  } catch (e) {
    throw new Error(`bad json: ${(e as Error).message}`);
  }
  if (typeof v !== "object" || v === null || Array.isArray(v)) throw new Error("bad json");
  const text = (key: string, max: number): string => {
    const s = v[key];
    if (typeof s !== "string") throw new Error(`missing ${key}`);
    if (!(s.length > 0 && s.length <= max && /^[A-Za-z0-9._-]+$/.test(s))) throw new Error(`invalid ${key}`);
    return s;
  };
  // Missing counts are 0 (clean events have no category breakdown); present ones must be non-negative integers.
  const count = (key: string): number => {
    const n = v[key];
    if (n === undefined || n === null) return 0;
    if (!Number.isSafeInteger(n) || n < 0) throw new Error(`invalid ${key}`);
    return n;
  };
  const kind = text("kind", 16);
  if (!KINDS.includes(kind)) throw new Error("invalid kind");
  // Sent by apps from 0.4 on; older ones leave them out.
  const optional = (key: string, allowed: RegExp): string | null => {
    const s = v[key];
    if (s === undefined || s === null || s === "") return null;
    if (typeof s !== "string" || !allowed.test(s)) throw new Error(`invalid ${key}`);
    return s;
  };
  const sizes = SIZE_FIELDS.map(count);
  // Free text from the OS (model names); shown escaped, so only length and control characters are limited.
  let specs: string | null = null;
  if (v.specs !== undefined && v.specs !== null) {
    if (!(Array.isArray(v.specs) && kind === "open" && v.specs.length <= MAX_SPECS)) throw new Error("invalid specs");
    specs = JSON.stringify(
      v.specs.map((spec: any) => {
        const obj: Record<string, string> = {};
        for (const key of ["kind", "brand", "name", "detail"]) {
          const s = spec?.[key];
          if (typeof s !== "string" || [...s].length > MAX_SPEC_TEXT || /\p{Cc}/u.test(s)) throw new Error("invalid specs");
          obj[key] = s;
        }
        return obj;
      }),
    );
  }
  return {
    install_id: text("install_id", 64),
    version: text("version", 32),
    os: text("os", 32),
    client: text("client", 16),
    kind,
    files: count("files"),
    sizes,
    specs,
    plan: optional("plan", /^(free|pro)$/),
    channel: optional("channel", /^(web|appstore|playstore)$/),
    detail: optional("detail", /^[a-z_]{1,24}$/),
  };
}

async function insert(db: D1Database, e: Event, country: string | null): Promise<void> {
  const now = Math.floor(Date.now() / 1000);
  const stmts = [
    db
      .prepare(
        `INSERT INTO events(ts, install_id, version, os, client, kind, files, bytes, duplicate_bytes, junk_bytes, old_download_bytes, large_bytes,
                            country, plan, channel, detail)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)`,
      )
      .bind(now, e.install_id, e.version, e.os, e.client, e.kind, e.files, ...e.sizes, country, e.plan, e.channel, e.detail),
  ];
  if (e.specs !== null) {
    stmts.push(
      db
        .prepare(
          `INSERT INTO hardware(install_id, ts, specs) VALUES (?1, ?2, ?3)
           ON CONFLICT(install_id) DO UPDATE SET ts = excluded.ts, specs = excluded.specs`,
        )
        .bind(e.install_id, now, e.specs),
    );
  }
  await db.batch(stmts);
}
