//! Self-update for the website builds (Mac DMG, Windows and Linux archives); the Mac App Store updates its build itself.
//!
//! `{site}/update/<platform>.json` names that platform's newest version, its file and that file's SHA-256 (older Macs
//! read `{site}/update.json`). Each is signed with deskMonk's release key (Ed25519, `<file>.sig`); the app holds only the public half below, so a
//! manifest anyone else wrote is refused, and so is any download whose hash differs. A version is only ever replaced
//! by a newer one. On macOS the new app must also carry Apple's Developer ID signature of deskMonk's team.
//!
//! Installing swaps the program files in place and leaves the old ones next to them (removed at the next start); the
//! running copy keeps working until it is restarted. Settings, license and vaults live elsewhere and are not touched.
//! Where the files can't be replaced (a system package, an AppImage, the app run from the DMG) the user gets the
//! download page instead. Signing a release: `cargo run --example sign_update` (see examples/sign_update.rs).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The public half of deskMonk's update signing key (hex).
pub const PUBLIC_KEY: &str = "ba85eaf852249c3a698b360dff5fb87f3f6052d9d14fd8998ea84a7621716422";
/// Apple Developer ID team whose signature a downloaded Mac app must carry.
#[cfg(target_os = "macos")]
const TEAM_ID: &str = "LPP3L6Q6L8";

/// What a check found.
#[derive(Debug, Clone, PartialEq)]
pub enum Found {
    /// The new version is on disk; it runs after a restart.
    Installed(String),
    /// A newer version exists but this copy can't replace itself (or installing failed): download it by hand.
    Available(String),
    /// A newer version is being downloaded and installed right now.
    Downloading(String),
}

/// This platform's entry name in update.json, e.g. "macos", "windows-x86_64", "linux-aarch64".
pub fn platform() -> String {
    match std::env::consts::OS {
        "macos" => "macos".into(),
        os => format!("{os}-{}", std::env::consts::ARCH),
    }
}

/// Checks the site and, with `install` and when this copy can replace itself, downloads and installs the newer version;
/// `downloading` hears the version first, so the window can show it while that runs.
/// None: up to date, offline, no site configured, or the manifest isn't genuine.
pub fn check(install_it: bool, downloading: impl FnOnce(&str)) -> Option<Found> {
    let site = crate::plan::URL?;
    // Each platform's own file (its release machine writes only that one); update.json is the Macs' older one.
    let signed = |path: &str| Some((fetch(&format!("{site}{path}"), 64 << 10)?, fetch(&format!("{site}{path}.sig"), 1 << 10)?));
    let (manifest, sig) = signed(&format!("/update/{}.json", platform())).or_else(|| signed("/update.json"))?;
    let entry = entry_for(&manifest, &String::from_utf8_lossy(&sig), PUBLIC_KEY, &platform(), env!("CARGO_PKG_VERSION"))?;
    if installed_marker().is_some_and(|v| v == entry.version) {
        return Some(Found::Installed(entry.version));
    }
    let Some(target) = target().filter(|_| install_it) else { return Some(Found::Available(entry.version)) };
    downloading(&entry.version);
    let url = if entry.url.starts_with('/') { format!("{site}{}", entry.url) } else { entry.url.clone() };
    let installed = fetch(&url, 200 << 20)
        .filter(|bytes| hex(&sha256(bytes)) == entry.sha256.to_ascii_lowercase())
        .and_then(|bytes| install(&target, &bytes).map_err(|e| eprintln!("update: {e}")).ok());
    match installed {
        Some(()) => {
            remember_installed(&entry.version);
            Some(Found::Installed(entry.version))
        }
        None => Some(Found::Available(entry.version)),
    }
}

/// Starts the installed new version and ends this one.
pub fn restart() -> ! {
    if let Some(target) = target() {
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg("-n").arg(&target).spawn();
        #[cfg(not(target_os = "macos"))]
        let _ = std::process::Command::new(&target).spawn();
    }
    std::process::exit(0)
}

/// Removes what the last update left behind (the old program files). Called at startup.
pub fn tidy() {
    if let Some(target) = target() {
        let _ = fs::remove_dir_all(old_path(&target));
        let _ = fs::remove_file(old_path(&target));
        #[cfg(not(target_os = "macos"))]
        let _ = fs::remove_file(old_path(&sibling(&target, TUI)));
    }
    if installed_marker().is_some_and(|v| !is_newer(&v, env!("CARGO_PKG_VERSION"))) {
        let _ = fs::remove_file(marker_path().unwrap_or_default());
    }
}

// ---------- the manifest ----------

#[derive(Debug, PartialEq)]
pub struct Entry {
    pub version: String,
    pub url: String,
    pub sha256: String,
}

/// The platform's entry, if the signature is valid and its version is newer than `current`.
pub fn entry_for(manifest: &[u8], sig_hex: &str, public_key_hex: &str, platform: &str, current: &str) -> Option<Entry> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let key = VerifyingKey::from_bytes(&unhex(public_key_hex)?.try_into().ok()?).ok()?;
    let sig = Signature::from_bytes(&unhex(sig_hex.trim())?.try_into().ok()?);
    key.verify(manifest, &sig).ok()?;
    let v: serde_json::Value = serde_json::from_slice(manifest).ok()?;
    let e = &v[platform];
    let entry = Entry { version: e["version"].as_str()?.into(), url: e["url"].as_str()?.into(), sha256: e["sha256"].as_str()?.into() };
    is_newer(&entry.version, current).then_some(entry)
}

/// "0.10.0" > "0.9.1"; anything that isn't dotted numbers is never newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| v.split('.').map(|n| n.parse::<u32>().ok()).collect::<Option<Vec<_>>>();
    matches!((parse(latest), parse(current)), (Some(l), Some(c)) if l > c)
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).into()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn fetch(url: &str, limit: u64) -> Option<Vec<u8>> {
    let mut resp = ureq::get(url).config().timeout_global(Some(Duration::from_secs(600))).build().call().ok()?;
    let mut bytes = Vec::new();
    resp.body_mut().as_reader().take(limit).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

// ---------- where this copy lives, and replacing it ----------

#[cfg(windows)]
const TUI: &str = "deskmonk-tui.exe";
#[cfg(all(unix, not(target_os = "macos")))]
const TUI: &str = "deskmonk-tui";

/// What an update replaces: the .app bundle on macOS, the program file elsewhere. None when it can't be replaced
/// (App Store build, run from the DMG or a translocated copy, a system package, an AppImage, a folder not writable).
fn target() -> Option<PathBuf> {
    if cfg!(feature = "appstore") {
        return None;
    }
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    #[cfg(target_os = "macos")]
    let target = {
        let bundle = exe.parent()?.parent()?.parent()?.to_path_buf();
        let s = bundle.to_string_lossy();
        if !s.ends_with(".app") || s.starts_with("/Volumes/") || s.contains("/AppTranslocation/") {
            return None;
        }
        bundle
    };
    #[cfg(not(target_os = "macos"))]
    let target = {
        if std::env::var_os("APPIMAGE").is_some() {
            return None;
        }
        unold(&exe)
    };
    writable(target.parent()?).then_some(target)
}

/// `.<name>.old` → `<name>`: after an update, Linux's /proc/self/exe names the renamed old file, so "Restart now"
/// would start the old version again.
#[cfg(not(target_os = "macos"))]
fn unold(p: &Path) -> PathBuf {
    let name = p.file_name().unwrap_or_default().to_string_lossy();
    name.strip_prefix('.').and_then(|n| n.strip_suffix(".old")).map_or_else(|| p.to_path_buf(), |n| sibling(p, n))
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".deskmonk-write-test-{}", std::process::id()));
    let ok = fs::write(&probe, b"").is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

/// `deskMonk.app` → `.deskMonk.app.old`; `deskmonk.exe` → `.deskmonk.exe.old`.
fn old_path(p: &Path) -> PathBuf {
    p.with_file_name(format!(".{}.old", p.file_name().unwrap_or_default().to_string_lossy()))
}

fn new_path(p: &Path) -> PathBuf {
    p.with_file_name(format!(".{}.new", p.file_name().unwrap_or_default().to_string_lossy()))
}

#[cfg(not(target_os = "macos"))]
fn sibling(p: &Path, name: &str) -> PathBuf {
    p.with_file_name(name)
}

/// Moves `new` into `target`'s place; the old one goes to `.<name>.old` (a running program may be renamed, also on Windows).
fn swap(target: &Path, new: &Path) -> std::io::Result<()> {
    let old = old_path(target);
    let _ = fs::remove_dir_all(&old);
    let _ = fs::remove_file(&old);
    fs::rename(target, &old)?;
    if let Err(e) = fs::rename(new, target) {
        let _ = fs::rename(&old, target);
        return Err(e);
    }
    Ok(())
}

/// macOS: mounts the DMG, checks the app's Apple signature, copies it next to the running one and swaps them.
#[cfg(target_os = "macos")]
fn install(target: &Path, dmg: &[u8]) -> Result<(), String> {
    use std::process::Command;
    let work = std::env::temp_dir().join(format!("deskmonk-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let (image, mount) = (work.join("update.dmg"), work.join("mnt"));
    fs::write(&image, dmg).map_err(|e| e.to_string())?;
    let run = |cmd: &mut Command| cmd.status().map_err(|e| e.to_string()).and_then(|s| if s.success() { Ok(()) } else { Err(format!("{cmd:?} failed")) });
    run(Command::new("hdiutil").args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-quiet", "-mountpoint"]).arg(&mount).arg(&image))?;
    let result = (|| {
        let app = mount.join("deskMonk.app");
        let requirement = format!("=anchor apple generic and certificate leaf[subject.OU] = \"{TEAM_ID}\"");
        run(Command::new("codesign").args(["--verify", "--deep", "--strict", "-R"]).arg(&requirement).arg(&app))?;
        let staged = new_path(target);
        let _ = fs::remove_dir_all(&staged);
        run(Command::new("ditto").arg(&app).arg(&staged))?;
        swap(target, &staged).map_err(|e| e.to_string())
    })();
    let _ = Command::new("hdiutil").args(["detach", "-quiet"]).arg(&mount).status();
    let _ = fs::remove_dir_all(&work);
    result
}

/// Windows and Linux: the archive (.tar.gz) holds deskmonk and deskmonk-tui; each replaces the file of that name here.
#[cfg(not(target_os = "macos"))]
fn install(target: &Path, archive: &[u8]) -> Result<(), String> {
    let mut staged = Vec::new();
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for file in tar.entries().map_err(|e| e.to_string())? {
        let mut file = file.map_err(|e| e.to_string())?;
        let name = file.path().map_err(|e| e.to_string())?.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let main = target.file_name().unwrap_or_default().to_string_lossy() == name;
        if !main && name != TUI {
            continue;
        }
        let dest = sibling(target, &name);
        // Only replace a terminal version that is installed here too.
        if !main && !dest.exists() {
            continue;
        }
        let new = new_path(&dest);
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        fs::write(&new, bytes).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&new, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        }
        staged.push((dest, new, main));
    }
    if !staged.iter().any(|s| s.2) {
        return Err("the archive has no program for this platform".into());
    }
    for (dest, new, _) in &staged {
        swap(dest, new).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The version installed but not running yet, so a second check doesn't download it again.
fn marker_path() -> Option<PathBuf> {
    Some(crate::config_dir()?.join("update-installed"))
}

fn installed_marker() -> Option<String> {
    fs::read_to_string(marker_path()?).ok().map(|s| s.trim().to_string())
}

fn remember_installed(version: &str) {
    if let Some(p) = marker_path() {
        let _ = fs::create_dir_all(p.parent().unwrap());
        let _ = fs::write(p, version);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn restart_starts_the_new_file() {
        let old = old_path(Path::new("/home/u/.local/bin/deskmonk"));
        assert_eq!(unold(&old), Path::new("/home/u/.local/bin/deskmonk"));
        assert_eq!(unold(Path::new("/opt/deskmonk")), Path::new("/opt/deskmonk"));
    }

    #[test]
    fn newer_versions() {
        assert!(is_newer("0.5.0", "0.4.0"));
        assert!(is_newer("0.10.0", "0.9.1"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.4.0", "0.4.0"));
        assert!(!is_newer("0.3.9", "0.4.0"));
        assert!(!is_newer("<html>", "0.4.0"));
        assert!(!is_newer("", "0.4.0"));
    }

    #[test]
    fn only_a_signed_newer_manifest_counts() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let public = hex(key.verifying_key().as_bytes());
        let manifest = br#"{"macos":{"version":"0.6.0","url":"/downloads/deskMonk-0.6.0-macos.dmg","sha256":"ab"}}"#;
        let sig = hex(&key.sign(manifest).to_bytes());
        let entry = entry_for(manifest, &sig, &public, "macos", "0.5.0").unwrap();
        assert_eq!(entry.version, "0.6.0");
        assert_eq!(entry.url, "/downloads/deskMonk-0.6.0-macos.dmg");
        // Not newer, other platform, changed manifest, other key, broken signature: nothing.
        assert!(entry_for(manifest, &sig, &public, "macos", "0.6.0").is_none());
        assert!(entry_for(manifest, &sig, &public, "windows-x86_64", "0.5.0").is_none());
        let tampered = br#"{"macos":{"version":"0.6.0","url":"https://evil.example/x.dmg","sha256":"ab"}}"#;
        assert!(entry_for(tampered, &sig, &public, "macos", "0.5.0").is_none());
        let other = hex(SigningKey::from_bytes(&[8; 32]).verifying_key().as_bytes());
        assert!(entry_for(manifest, &sig, &other, "macos", "0.5.0").is_none());
        assert!(entry_for(manifest, "zz", &public, "macos", "0.5.0").is_none());
    }

    #[test]
    fn hex_round_trip() {
        assert_eq!(unhex(&hex(&[0, 1, 254, 255])), Some(vec![0, 1, 254, 255]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
        assert_eq!(hex(&sha256(b"abc"))[..8], *"ba7816bf");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn archive_replaces_the_programs() {
        let dir = std::env::temp_dir().join(format!("deskmonk-update-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(if cfg!(windows) { "deskmonk.exe" } else { "deskmonk" });
        fs::write(&exe, b"old").unwrap();
        fs::write(dir.join(TUI), b"old tui").unwrap();
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (name, body) in [(exe.file_name().unwrap().to_str().unwrap(), &b"new"[..]), (TUI, b"new tui"), ("README.md", b"x")] {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            tar.append_data(&mut h, format!("deskmonk-linux-x86_64/{name}"), body).unwrap();
        }
        let archive = tar.into_inner().unwrap().finish().unwrap();
        install(&exe, &archive).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        assert_eq!(fs::read(dir.join(TUI)).unwrap(), b"new tui");
        assert_eq!(fs::read(old_path(&exe)).unwrap(), b"old");
        assert!(!dir.join("README.md").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
