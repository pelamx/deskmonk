//! Named "release_manifests", not "update_…": Windows treats an executable whose name contains
//! "update" as an installer and refuses to run the test without elevation.

//! The update manifests in docs/ decide whether an installed copy ever hears about a new version.
//! Getting one wrong fails silently — `update::entry_for` uses `?` on each field, so a manifest missing
//! `url` or `sha256` makes the app find nothing and say nothing — which is how Windows and Linux sat on
//! "download"/"size" keys the app never reads, pointing at a .zip and an .AppImage that the installer
//! (tar + gzip) could not have unpacked anyway.

use std::path::{Path, PathBuf};

fn docs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("docs")
}

/// Every file the app would fetch: docs/update/<platform>.json, plus the older docs/update.json.
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

#[test]
fn every_manifest_names_a_file_that_is_there_and_matches_its_hash() {
    for (platform, path) in manifests() {
        let name = path.display().to_string();
        let text = std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let json: serde_json::Value = serde_json::from_slice(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        let entry = &json[&platform];
        assert!(!entry.is_null(), "{name}: has no \"{platform}\" key, so the app reads nothing from it");

        // The three fields update::entry_for insists on. "download" and "size" are never read.
        for field in ["version", "url", "sha256"] {
            assert!(entry[field].as_str().is_some(), "{name}: \"{platform}\" is missing \"{field}\"; the app would silently find no update");
        }

        let url = entry["url"].as_str().unwrap();
        assert!(url.starts_with('/'), "{name}: url {url} must be a site path");
        let file = docs().join(url.trim_start_matches('/'));
        assert!(file.is_file(), "{name}: points at {url}, which is not in docs/ — the download would 404");

        let bytes = std::fs::read(&file).unwrap();
        let got = deskmonk::update::hex(&deskmonk::update::sha256(&bytes));
        assert_eq!(got, entry["sha256"].as_str().unwrap().to_ascii_lowercase(), "{name}: sha256 does not match {url}; the download would be thrown away");

        // macOS installs from a disk image; everywhere else install() unpacks tar + gzip.
        if platform == "macos" {
            assert!(url.ends_with(".dmg"), "{name}: macOS installs from a .dmg, got {url}");
        } else {
            assert_eq!(&bytes[..2], b"\x1f\x8b", "{name}: {url} is not gzip ({:02x} {:02x}); install() would fail to unpack it", bytes[0], bytes[1]);
        }
    }
}

/// A manifest that claims a version older than this build would never offer an update to anyone on it.
#[test]
fn no_manifest_is_behind_the_version_in_cargo_toml() {
    for (platform, path) in manifests() {
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let Some(version) = json[&platform]["version"].as_str() else { continue }; // reported by the test above
        assert!(
            !deskmonk::update::is_newer(env!("CARGO_PKG_VERSION"), version),
            "{}: offers {version}, older than the {} in Cargo.toml — a release that stops halfway leaves everyone on the old build",
            path.display(),
            env!("CARGO_PKG_VERSION"),
        );
    }
}
