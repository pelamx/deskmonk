# deskMonk

Always write "deskMonk" in visible text (lowercase only in identifiers and file names).

## Every interface shows its version

The desktop app's top bar, its window titles, the menu bar / tray menu, the terminal version's header and the phone
app all show the version number, read from the build (`CARGO_PKG_VERSION`, `PackageInfo`), never typed in. Any new
window or screen shows it too, without being asked.

## The icon stays in the Dock while deskMonk is open

As long as deskMonk runs, its icon is in the Dock / taskbar, also while the window is hidden: macOS hides the app
on close (never an accessory app without a Dock icon), and on Linux the window's app id `deskmonk` must match an
installed `deskmonk.desktop` (packages and install.sh install one; the AppImage adds its own on start).

## Releasing: one machine per platform

The Mac builds the Mac app, a Windows PC the Windows app, a Linux PC the Linux app. All three publish to the same
site, so each one touches **only its own platform's files**:

| Platform | Website download | Release file (signed) |
|---|---|---|
| macos | `docs/downloads/deskMonk-<v>-macos.dmg` (notarized, from `packaging/macos.sh`) | `docs/update/macos.json` |
| windows-x86_64 | `docs/downloads/deskMonk-<v>-windows-x86_64.zip` | `docs/update/windows-x86_64.json` |
| linux-x86_64 | `docs/downloads/deskMonk-<v>-linux-x86_64.AppImage` | `docs/update/linux-x86_64.json` |

Each release also puts its platform's entry into the shared `docs/update.json`, keeping the others' entries, because
apps before 0.5.8 (every platform) read only that file. Nothing else names a version or a download file: the website's buttons and old download links come from the release
files (`worker/src/site.ts`), and installed apps read their own platform's file (`src/update.rs`). Never edit
`docs/index.html` or `docs/_redirects` for a release, and never edit another platform's files.

"commit push deploy" means a release of this machine's platform:

1. `git pull --rebase` first. The version in `Cargo.toml` must be higher than every platform's current one (look at
   all `docs/update/*.json` and `git tag`), so two machines never ship different builds under one number. Bump it,
   commit, push, and tag that commit `v<version>`; every platform builds that version from that commit.
2. Build this platform's app from that commit, with `DESKMONK_TELEMETRY_URL=https://deskmonk.app/api/events` and
   `DESKMONK_LICENSE_URL=https://deskmonk.app` set. Mac: `packaging/macos.sh` (signed and notarized; CI's ad-hoc DMG
   never goes on the site).
   Windows: `packaging/sign.ps1 <zip>` signs the .exe files inside the zip (and the same for the update archive's
   contents) **before** step 3 — the sha256 in the release file must be of the signed bytes, or every self-update
   downloads a file whose hash no longer matches and silently gives up.
3. `packaging/publish-update.sh <platform> <version> <download> [<update archive>]` copies the files into `docs/`,
   removes this platform's previous ones, and writes and signs its release file. It needs the release key
   `~/.deskmonk-release/update-signing.key` (the same file on all three machines; never commit it).
4. `git add docs && git commit && git pull --rebase && git push`, then `npx wrangler deploy` in `worker/`.

Every deploy first runs `worker/predeploy.mjs` (wrangler's build step). It refuses to deploy anything that is not
exactly `origin/master` (uncommitted, unpushed, or behind), and any release file that is unsigned, names a missing
file, or has a wrong sha256; on a Mac also a Mac download that is not notarized. CI runs the same checks on every
push. Don't work around it: fix what it reports. `DESKMONK_DEPLOY_ANYWAY=1` skips only the git part, for
`wrangler dev` or an emergency.

## Tests

`cargo test` runs the real GUI invisibly (`src/gui_tests.rs`). On a Mac, set `DESKMONK_TEST_HOME` to a folder on a
throwaway disk image so its test files don't land in the real Trash. Never drive the GUI with synthetic keyboard or
mouse input. Start a release build only with `DESKMONK_NO_STATS=1` set (and a throwaway HOME), so the test run sends
no stats; every start of a release build otherwise counts as a new installation in /admin. A test run that did get
counted is marked "mine" in /admin. The website: `npm test` in `worker/`.
