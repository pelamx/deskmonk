#!/usr/bin/env bash
# Publishes ONE platform's release on deskmonk.app, touching only that platform's files, so the Mac, Windows and
# Linux machines can each release on their own without breaking the others (see CLAUDE.md, "Releasing"):
#   packaging/publish-update.sh macos          0.5.7 dist/deskMonk-0.5.7-macos.dmg
#   packaging/publish-update.sh windows-x86_64 0.5.7 dist/deskmonk-windows-x86_64.zip  dist/deskmonk-windows-x86_64.tar.gz
#   packaging/publish-update.sh linux-x86_64   0.5.7 dist/deskMonk-0.5.7-linux-x86_64.AppImage dist/deskmonk-linux-x86_64.tar.gz
# Arguments: platform, version, the website download, then the file installed copies update from (the Mac DMG is both).
# Without the update file, the website gets the new download but installed copies are not told.
# It copies the files to docs/downloads (versioned names), removes this platform's previous ones, writes and signs
# docs/update/<platform>.json, and adds this platform's entry to docs/update.json (what apps before 0.5.8 read). Then commit, push,
# and deploy (`npx wrangler deploy` in worker/; its build step refuses anything not pushed or not consistent).
# Needs the release key (~/.deskmonk-release/update-signing.key; the same file on every release machine).
set -euo pipefail
cd "$(dirname "$0")/.."
[ $# -ge 3 ] && [ $# -le 4 ] || { sed -n 2,13p "$0"; exit 2; }
PLATFORM=$1 VERSION=$2 DOWNLOAD=$3 UPDATE=${4:-}
case $PLATFORM in macos | windows-x86_64 | linux-x86_64 | linux-aarch64) ;; *) echo "unknown platform $PLATFORM" >&2; exit 2 ;; esac
[ "$PLATFORM" = macos ] && UPDATE=$DOWNLOAD
KEY=${DESKMONK_UPDATE_KEY:-$HOME/.deskmonk-release/update-signing.key}
[ -f "$KEY" ] || { echo "no release key at $KEY" >&2; exit 1; }
[ -f "$DOWNLOAD" ] || { echo "no file $DOWNLOAD" >&2; exit 1; }
if [ "$PLATFORM" = macos ] && [ "$(uname)" = Darwin ]; then
  # A Mac download must open without warnings: CI's ad-hoc signed DMG never goes on the site.
  xcrun stapler validate "$DOWNLOAD" >/dev/null || { echo "$DOWNLOAD is not notarized; build it with packaging/macos.sh on the Mac" >&2; exit 1; }
fi

# Versioned names, so a browser or CDN never serves an old file under a new release's link.
name() { local base ext; base=$(basename "$1"); ext=${base##*.}; [ "$base" = "${base%.tar.gz}" ] || ext=tar.gz
  echo "deskMonk-$VERSION-$PLATFORM.$ext"; }
JSON=docs/update/$PLATFORM.json
mkdir -p docs/downloads/update docs/update
OLD=$(python3 -c 'import json,sys; e=json.load(open(sys.argv[1]))[sys.argv[2]]; print(e.get("download","")); print(e.get("url",""))' "$JSON" "$PLATFORM" 2>/dev/null || true)
DL=docs/downloads/$(name "$DOWNLOAD")
cp "$DOWNLOAD" "$DL"
UP=""
if [ -n "$UPDATE" ]; then
  if [ "$UPDATE" = "$DOWNLOAD" ]; then UP=$DL; else UP=docs/downloads/update/$(name "$UPDATE"); cp "$UPDATE" "$UP"; fi
fi
python3 - "$PLATFORM" "$VERSION" "$DL" "$UP" "$JSON" <<'PY'
import hashlib, json, os, sys
platform, version, dl, up, path = sys.argv[1:]
e = {"version": version, "download": "/" + os.path.relpath(dl, "docs"), "size": os.path.getsize(dl)}
if up:
    e["url"] = "/" + os.path.relpath(up, "docs")
    e["sha256"] = hashlib.sha256(open(up, "rb").read()).hexdigest()
open(path, "w").write(json.dumps({platform: e}, indent=2, sort_keys=True) + "\n")
PY
# This platform's previous files, when the new ones have other names.
for f in $OLD; do [ -n "$f" ] && [ "docs$f" != "$DL" ] && [ "docs$f" != "$UP" ] && rm -f "docs$f"; done
sign() { PATH=$HOME/.cargo/bin:$PATH cargo run -q --example sign_update -- sign "$KEY" "$1"; }
sign "$JSON"
if [ -n "$UP" ]; then
  # Apps before 0.5.8 (every platform) only read docs/update.json: this platform's entry goes there too, the others' stay.
  python3 -c 'import json,sys; p=sys.argv[2]; e=json.load(open(sys.argv[1]))[p]; m=json.load(open("docs/update.json")); m[p]={k: e[k] for k in ("sha256","url","version")}; open("docs/update.json","w").write(json.dumps(m, indent=2, sort_keys=True)+"\n")' "$JSON" "$PLATFORM"
  sign docs/update.json
fi
node worker/predeploy.mjs --check-only
echo "$PLATFORM → $VERSION: $DL${UP:+, update $UP}. Now: git add docs && git commit && git push, then deploy."
