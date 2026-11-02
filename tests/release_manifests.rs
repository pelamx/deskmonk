//! Named "release_manifests", not "update_…": Windows treats an executable whose name contains
//! "update" as an installer and refuses to run the test without elevation.
//!
//! docs/update/<platform>.json is read by two different things, and each ignores the other's fields:
//! the website (worker/src/site.ts) sends visitors to `download` and prints `version` and `size` on the
//! button, while an installed copy (src/update.rs) fetches `url` and refuses anything whose `sha256`
//! does not match. Drop a field and that half goes quiet — the site 404s, or the app finds no update
//! and says nothing, because `entry_for` takes each field with `?`. Both halves are checked here.

use std::path::{Path, PathBuf};

fn docs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("docs")
}

/// Every manifest the site or an installed copy can fetch.
fn manifests() -> Vec<(String, PathBuf)> {
    let mut out = vec![("macos".into(), docs().join("update.json"))];
    for entry in std::fs::read_dir(docs().join("update")).expect("docs/update exists").flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            out.push((path.file_stem().unwrap().to_string_lossy().into_owned(), path));
        }
    }
    out
}

fn site_file(url: &str) -> PathBuf {
    docs().join(url.trim_start_matches('/'))
}

#[test]
fn the_website_can_hand_out_every_platforms_download() {
    for (platform, path) in manifests() {
        // The site reads only the per-platform files; update.json is the older one, for copies before 0.5.8.
        if path.file_name().is_some_and(|n| n == "update.json") {
            continue;
        }
        let name = path.display().to_string();
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let entry = &json[&platform];
        assert!(!entry.is_null(), "{name}: has no \"{platform}\" key, so the site drops the platform and its button 404s");

        let download = entry["download"].as_str().unwrap_or_else(|| panic!("{name}: no \"download\"; the site's button would 404"));
        assert!(download.starts_with("/downloads/"), "{name}: the site only follows /downloads/ paths, got {download}");
        let file = site_file(download);
        assert!(file.is_file(), "{name}: download {download} is not in docs/");
        let size = entry["size"].as_u64().unwrap_or_else(|| panic!("{name}: no \"size\"; the button's label needs it"));
        assert_eq!(size, file.metadata().unwrap().len(), "{name}: \"size\" does not match {download}");
        assert!(entry["version"].as_str().is_some(), "{name}: no \"version\"");
    }
}

#[test]
fn an_installed_copy_can_fetch_and_verify_every_update_it_is_offered() {
    for (platform, path) in manifests() {
        let name = path.display().to_string();
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let entry = &json[&platform];
        // A platform with no update file yet is a gap, not a lie; only a half-written one breaks.
        let (Some(url), sha) = (entry["url"].as_str(), entry["sha256"].as_str()) else {
            assert!(entry["sha256"].is_null(), "{name}: has \"sha256\" but no \"url\"; update::entry_for needs both or neither");
            continue;
        };
        let sha = sha.unwrap_or_else(|| panic!("{name}: has \"url\" but no \"sha256\"; update::entry_for returns None and the app stays silent"));
        let file = site_file(url);
        assert!(file.is_file(), "{name}: url {url} is not in docs/ — the download would 404");

        let bytes = std::fs::read(&file).unwrap();
        assert_eq!(deskmonk::update::hex(&deskmonk::update::sha256(&bytes)), sha.to_ascii_lowercase(), "{name}: sha256 does not match {url}; the download would be thrown away");

        // macOS installs from a disk image; everywhere else install() unpacks tar + gzip.
        if platform == "macos" {
            assert!(url.ends_with(".dmg"), "{name}: macOS installs from a .dmg, got {url}");
        } else {
            assert_eq!(&bytes[..2], b"\x1f\x8b", "{name}: {url} is not gzip ({:02x} {:02x}); install() cannot unpack it", bytes[0], bytes[1]);
        }
    }
}

/// A manifest may lag Cargo.toml — the version is bumped long before the release is built, and that is
/// not a broken state. Claiming a version that does not exist yet is: every installed copy would chase
/// a file that was never published.
#[test]
fn no_manifest_offers_a_version_that_was_never_built() {
    for (platform, path) in manifests() {
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let Some(version) = json[&platform]["version"].as_str() else { continue }; // reported by the test above
        assert!(
            !deskmonk::update::is_newer(version, env!("CARGO_PKG_VERSION")),
            "{}: offers {version}, which is newer than the {} this repo builds",
            path.display(),
            env!("CARGO_PKG_VERSION"),
        );
    }
}
