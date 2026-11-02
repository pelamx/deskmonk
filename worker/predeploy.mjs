// Runs before every `wrangler deploy` (wrangler.toml [build]), on whichever machine deploys, so one platform's
// release can't break another's downloads or updates:
//  1. what goes live is exactly origin/master: nothing uncommitted under docs/ or worker/, nothing unpushed, nothing
//     newer on GitHub (a deploy from an old checkout would take other machines' releases off the site);
//  2. every platform's docs/update/<platform>.json (and the Macs' old docs/update.json) is signed with the release key,
//     names files that exist, and its sha256 matches; every /downloads/ link in the pages exists;
//  3. on a Mac, the Mac download is notarized.
// `node worker/predeploy.mjs --check-only` runs 2 and 3 (publish-update.sh, CI). DESKMONK_DEPLOY_ANYWAY=1 skips 1,
// for `wrangler dev` or an emergency; say why in the next commit.
import { createHash, createPublicKey, verify } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const docs = join(root, "docs");
const PLATFORMS = ["macos", "windows-x86_64", "linux-x86_64"]; // the website's buttons (worker/src/site.ts)
const problems = [];
const git = (...args) => execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();

if (!process.argv.includes("--check-only") && process.env.DESKMONK_DEPLOY_ANYWAY !== "1") {
  git("fetch", "-q", "origin");
  const dirty = git("status", "--porcelain", "--", "docs", "worker");
  if (dirty) problems.push(`uncommitted changes would go live; commit and push them first:\n${dirty}`);
  const [head, remote] = [git("rev-parse", "HEAD"), git("rev-parse", "origin/master")];
  if (head !== remote) {
    const behind = git("rev-list", "--count", "HEAD..origin/master");
    const ahead = git("rev-list", "--count", "origin/master..HEAD");
    problems.push(`this checkout is not origin/master (${behind} behind, ${ahead} ahead): pull --rebase and push, then deploy`);
  }
}

const hex = readFileSync(join(root, "src/update.rs"), "utf8").match(/PUBLIC_KEY: &str = "([0-9a-f]{64})"/)?.[1];
const key = createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x: Buffer.from(hex ?? "", "hex").toString("base64url") }, format: "jwk" });
const signed = (file) => {
  const bytes = readFileSync(file);
  const sig = existsSync(`${file}.sig`) ? Buffer.from(readFileSync(`${file}.sig`, "utf8").trim(), "hex") : Buffer.alloc(0);
  if (sig.length !== 64 || !verify(null, bytes, key, sig)) problems.push(`${file}: signature missing or wrong; sign it with packaging/publish-update.sh`);
  return JSON.parse(bytes);
};
const file = (url) => join(docs, url.replace(/^\//, ""));
const sha256 = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");

for (const p of PLATFORMS) {
  const path = join(docs, "update", `${p}.json`);
  if (!existsSync(path)) { problems.push(`${path} is missing: the ${p} download button has nothing to show`); continue; }
  const e = signed(path)[p];
  if (!e?.version || !e?.download) { problems.push(`${path}: needs a "${p}" entry with version and download`); continue; }
  if (!existsSync(file(e.download))) problems.push(`${p}: download ${e.download} is not in docs/`);
  if (e.url) {
    if (!existsSync(file(e.url))) problems.push(`${p}: update file ${e.url} is not in docs/`);
    else if (sha256(file(e.url)) !== e.sha256) problems.push(`${p}: sha256 of ${e.url} does not match`);
  }
  if (p === "macos" && process.platform === "darwin" && existsSync(file(e.download))) {
    try { execFileSync("xcrun", ["stapler", "validate", file(e.download)], { stdio: "ignore" }); }
    catch { problems.push(`macos: ${e.download} is not notarized (CI's DMG must not go on the site; build with packaging/macos.sh)`); }
  }
}
// Macs before 0.5.8 read this one; it must say what update/macos.json says.
const legacy = signed(join(docs, "update.json")).macos;
const mac = existsSync(join(docs, "update/macos.json")) ? JSON.parse(readFileSync(join(docs, "update/macos.json"))).macos : null;
if (mac && (legacy?.version !== mac.version || legacy?.url !== mac.url || legacy?.sha256 !== mac.sha256)) {
  problems.push("docs/update.json does not match docs/update/macos.json; run packaging/publish-update.sh macos …");
}
for (const page of readdirSync(docs).filter((f) => f.endsWith(".html"))) {
  for (const [, url] of readFileSync(join(docs, page), "utf8").matchAll(/href="(\/downloads\/[^"?#]+)"/g)) {
    if (!existsSync(file(url))) problems.push(`docs/${page} links to ${url}, which is not there`);
  }
}

if (problems.length) {
  console.error(`deskMonk deploy check failed:\n- ${problems.join("\n- ")}`);
  process.exit(1);
}
console.log("deskMonk deploy check: ok");
