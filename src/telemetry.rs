//! Anonymous usage stats. Never sends file names or paths, only counts, byte totals and hardware specs.
//! Every copy sends one basic "open" per start (random ID, version, OS, plan) so installs and active users can
//! be counted; everything else (hardware, scans, cleaning, features) only after the user agreed.
//! Disabled entirely unless built with `DESKMONK_TELEMETRY_URL=https://host/api/events`.

use crate::hardware::Component;
use crate::scan::{Category, Item};
use std::fs;
use std::path::PathBuf;

pub const URL: Option<&str> = option_env!("DESKMONK_TELEMETRY_URL");

#[derive(Clone)]
pub struct Telemetry {
    pub install_id: String,
    /// None = user has not been asked yet.
    pub consent: Option<bool>,
}

fn config_path() -> Option<PathBuf> {
    Some(crate::config_dir()?.join("telemetry"))
}

impl Telemetry {
    /// File format: line 1 install id, line 2 "yes-hw"/"no" (absent = not asked).
    /// An old "yes" was given before hardware specs were collected, so the user is asked again.
    pub fn load() -> Self {
        let text = config_path()
            .and_then(|p| fs::read_to_string(p).ok())
            .unwrap_or_default();
        let mut lines = text.lines();
        let install_id = lines.next().filter(|l| !l.is_empty()).map(str::to_owned);
        let consent = match lines.next() {
            Some("yes-hw") => Some(true),
            Some("no") => Some(false),
            _ => None,
        };
        // Saved at once, not only with the consent answer: the license server counts devices by this id,
        // so a new one on every launch would use up a key's devices.
        let install_id = install_id.unwrap_or_else(|| {
            let id = uuid::Uuid::new_v4().to_string();
            if let Some(path) = config_path() {
                let _ = fs::create_dir_all(path.parent().unwrap());
                let _ = fs::write(path, format!("{id}\n"));
            }
            id
        });
        Self { install_id, consent }
    }

    /// The basic ping at every start, from users who haven't agreed to the full stats; those who agreed send
    /// the full "open" (with hardware) instead. shortcut: a user who says yes in the first session is counted
    /// as opening twice that day, which doesn't change installs or active users.
    pub fn started(&self, client: &str) {
        if self.consent != Some(true) {
            self.post(serde_json::json!({ "kind": "open", "client": client, "detail": "basic" }));
        }
    }

    pub fn set_consent(&mut self, yes: bool) {
        self.consent = Some(yes);
        if let Some(path) = config_path() {
            let _ = fs::create_dir_all(path.parent().unwrap());
            let _ = fs::write(
                path,
                format!("{}\n{}\n", self.install_id, if yes { "yes-hw" } else { "no" }),
            );
        }
    }

    /// True when the app should ask for consent.
    pub fn needs_prompt(&self) -> bool {
        URL.is_some() && self.consent.is_none()
    }

    pub fn scan_done(&self, client: &str, files_scanned: usize, items: &[Item]) {
        let bytes = |c: Category| {
            items
                .iter()
                .filter(|i| i.category == c)
                .map(|i| i.size)
                .sum::<u64>()
        };
        self.send(serde_json::json!({
            "kind": "scan",
            "client": client,
            "files": files_scanned,
            "bytes": items.iter().map(|i| i.size).sum::<u64>(),
            "duplicate_bytes": bytes(Category::Duplicate),
            "junk_bytes": bytes(Category::Junk),
            "old_download_bytes": bytes(Category::OldDownload),
            "large_bytes": bytes(Category::Large),
        }));
    }

    /// Sent once per app start: counts as a usage session and keeps the admin panel's hardware list current.
    pub fn opened(&self, client: &str, specs: &[Component]) {
        let specs: Vec<_> = specs
            .iter()
            .map(|c| serde_json::json!({ "kind": c.kind, "brand": c.brand, "name": c.name, "detail": c.detail }))
            .collect();
        self.send(serde_json::json!({ "kind": "open", "client": client, "specs": specs }));
    }

    pub fn clean_done(&self, client: &str, files: usize, bytes: u64) {
        self.send(serde_json::json!({ "kind": "clean", "client": client, "files": files, "bytes": bytes }));
    }

    /// A feature was used: "vault" (detail "encrypt"/"open"), "focus" (a focus session finished),
    /// "pro" (the Pro dialog opened; detail = which button or limit opened it), "bought" (detail = store).
    pub fn used(&self, client: &str, kind: &str, detail: &str) {
        self.send(serde_json::json!({ "kind": kind, "client": client, "detail": detail }));
    }

    fn send(&self, event: serde_json::Value) {
        if self.consent == Some(true) {
            self.post(event);
        }
    }

    fn post(&self, mut event: serde_json::Value) {
        // DESKMONK_NO_STATS=1: test runs of release builds (packaging checks, CI) send nothing, so /admin counts only real users.
        let Some(url) = URL.filter(|_| std::env::var_os("DESKMONK_NO_STATS").is_none()) else {
            return;
        };
        event["install_id"] = self.install_id.clone().into();
        event["version"] = env!("CARGO_PKG_VERSION").into();
        event["os"] = std::env::consts::OS.into();
        event["plan"] = if crate::plan::Plan::load().is_pro() { "pro" } else { "free" }.into();
        event["channel"] = if cfg!(feature = "appstore") { "appstore" } else { "web" }.into();
        // Fire and forget: stats must never block or break cleaning.
        std::thread::spawn(move || {
            let _ = ureq::post(url)
                .header("Content-Type", "application/json")
                .send(event.to_string());
        });
    }
}
