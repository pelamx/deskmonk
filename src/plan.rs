//! Free plan limits and the Pro subscription. Pro is sold through Paddle; the license server (worker/)
//! says until when a key or installation is paid for. Free usage is counted per local day.

use crate::i18n::t;
use crate::tf;
use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// License server base URL, set at build time; without it (dev builds) everything is Pro.
/// CI passes an unset repository variable as "", which counts as not set.
/// The Mac App Store build sells Pro through Apple instead (src/store.rs), so it has no license server.
pub const URL: Option<&str> = match option_env!("DESKMONK_LICENSE_URL") {
    Some(url) if !url.is_empty() && !cfg!(feature = "appstore") => Some(url),
    _ => None,
};

/// Prices shown in the app; the amounts charged are set on the Paddle prices and must match these.
pub const PRICE_MONTHLY: &str = "$2.50";
pub const PRICE_YEARLY: &str = "$25";

pub const FREE_CLEAN: u64 = 2 << 30;
pub const FREE_VAULTS: u32 = 3;
pub const FREE_VAULT_SIZE: u64 = 200 << 20;
pub const FREE_LOCKED_APPS: usize = 1;
/// Pro keeps working this long past the paid period, e.g. offline or while a renewal goes through.
const GRACE: u64 = 7 * 86400;

/// File format: key, pro_until, day, cleaned bytes, vault count; one per line.
#[derive(Default, Debug, PartialEq)]
pub struct Plan {
    pub key: String,
    /// End of the paid period (unix seconds); 0 = never paid.
    pub pro_until: u64,
    day: String,
    cleaned: u64,
    vaults: u32,
}

fn path() -> Option<std::path::PathBuf> {
    Some(crate::config_dir()?.join("plan"))
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Plan {
    pub fn load() -> Self {
        Self::parse(&path().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default())
    }

    /// A missing or unreadable line keeps its default, so a file from an older version, a truncated
    /// one or no file at all still loads — as Free, never as Pro.
    fn parse(text: &str) -> Self {
        let mut l = text.lines();
        let mut next = || l.next().unwrap_or_default().to_string();
        Self {
            key: next(),
            pro_until: next().parse().unwrap_or(0),
            day: next(),
            cleaned: next().parse().unwrap_or(0),
            vaults: next().parse().unwrap_or(0),
        }
    }

    fn text(&self) -> String {
        format!("{}\n{}\n{}\n{}\n{}\n", self.key, self.pro_until, self.day, self.cleaned, self.vaults)
    }

    fn save(&self) {
        let Some(path) = path() else { return };
        let _ = fs::create_dir_all(path.parent().unwrap());
        let _ = fs::write(&path, self.text());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
    }

    pub fn is_pro(&self) -> bool {
        (URL.is_none() && !cfg!(feature = "appstore")) || self.pro_at(now())
    }

    fn pro_at(&self, now: u64) -> bool {
        self.pro_until > 0 && now <= self.pro_until + GRACE
    }

    /// Starts a new day's counts when the date changed.
    fn roll(&mut self, day: String) {
        if self.day != day {
            *self = Self { key: std::mem::take(&mut self.key), pro_until: self.pro_until, day, ..Default::default() };
        }
    }

    /// Bytes the free plan may still clean today; None = unlimited.
    pub fn clean_left(&mut self) -> Option<u64> {
        if self.is_pro() {
            return None;
        }
        self.roll(today());
        Some(FREE_CLEAN.saturating_sub(self.cleaned))
    }

    pub fn record_clean(&mut self, bytes: u64) {
        self.roll(today());
        self.cleaned += bytes;
        self.save();
    }

    /// Encryptions the free plan may still do today; None = unlimited.
    pub fn vaults_left(&mut self) -> Option<u32> {
        if self.is_pro() {
            return None;
        }
        self.roll(today());
        Some(FREE_VAULTS.saturating_sub(self.vaults))
    }

    pub fn record_vault(&mut self) {
        self.roll(today());
        self.vaults += 1;
        self.save();
    }

    pub fn can_lock_more(&self, locked: usize) -> bool {
        self.is_pro() || locked < FREE_LOCKED_APPS
    }

    /// Stores what the license server answered.
    pub fn set_license(&mut self, key: String, pro_until: u64) {
        self.key = key;
        self.pro_until = pro_until;
        self.save();
    }
}

/// The free plan's error for a vault that is too big.
pub fn vault_too_big(size: u64) -> String {
    use crate::scan::human_size;
    tf!(
        "Ücretsiz plan en fazla {} şifreler; bu {}. Pro ile sınır yok.",
        "The free plan encrypts up to {}; this is {}. Pro has no limit.",
        "El plan gratuito cifra hasta {}; esto ocupa {}. Pro no tiene límite.",
        human_size(FREE_VAULT_SIZE),
        human_size(size)
    )
}

/// Asks the license server about `key` (or, with no key, a purchase made from this installation).
/// Returns the key to keep and the end of the paid period (0 = not Pro).
pub fn validate(key: &str, install_id: &str) -> Result<(String, u64), String> {
    let v = post("/api/license/validate", serde_json::json!({ "key": key, "install_id": install_id }))?;
    Ok((v["key"].as_str().unwrap_or(key).to_string(), v["pro_until"].as_u64().unwrap_or(0)))
}

/// Paddle's page to cancel or change the subscription.
pub fn portal_url(key: &str) -> Result<String, String> {
    let v = post("/api/license/portal", serde_json::json!({ "key": key }))?;
    v["url"].as_str().map(str::to_string).ok_or_else(|| t("Sunucu yanıtı geçersiz", "Invalid server reply", "Respuesta del servidor no válida").into())
}

/// Checkout page for this installation; `yearly` picks the plan.
pub fn buy_url(install_id: &str, yearly: bool) -> Option<String> {
    Some(format!("{}/buy?install={install_id}&plan={}", URL?, if yearly { "yearly" } else { "monthly" }))
}

fn post(route: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
    let base = URL.ok_or("no license server")?;
    let offline = |e: ureq::Error| tf!("Sunucuya ulaşılamadı: {e}", "Could not reach the server: {e}", "No se pudo contactar con el servidor: {e}");
    let mut resp = ureq::post(format!("{base}{route}"))
        .config()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(offline)?;
    let text = resp.body_mut().read_to_string().map_err(offline)?;
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    match v["error"].as_str() {
        None if resp.status().is_success() => Ok(v),
        Some("unknown_key") => Err(t("Lisans anahtarı bulunamadı", "License key not found", "Clave de licencia no encontrada").into()),
        Some("too_many_devices") => Err(t(
            "Bu anahtar zaten 3 bilgisayarda kullanılıyor",
            "This key is already used on 3 computers",
            "Esta clave ya se usa en 3 ordenadores",
        ).into()),
        _ => Err(tf!("Sunucu hatası ({})", "Server error ({})", "Error del servidor ({})", resp.status())),
    }
}

/// Local date as YYYY-MM-DD.
pub fn today() -> String {
    date(now())
}

/// Unix seconds as a local YYYY-MM-DD.
pub fn date(secs: u64) -> String {
    civil((secs as i64 + utc_offset(secs)).div_euclid(86400))
}

#[cfg(unix)]
fn utc_offset(secs: u64) -> i64 {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() { 0 } else { tm.tm_gmtoff as i64 }
}

// shortcut: Windows counts days in UTC; GetTimeZoneInformation would make it local.
#[cfg(not(unix))]
fn utc_offset(_: u64) -> i64 {
    0
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + i64::from(m <= 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(civil(0), "1970-01-01");
        assert_eq!(civil(19_782), "2024-02-29");
        assert_eq!(civil(20_755), "2026-10-29");
    }

    #[test]
    fn grace_and_daily_counts() {
        let mut plan = Plan::default();
        assert!(!plan.pro_at(1000));
        plan.pro_until = 1000;
        assert!(plan.pro_at(1000 + GRACE) && !plan.pro_at(1001 + GRACE));

        plan.roll("2026-10-29".into());
        plan.cleaned = FREE_CLEAN;
        plan.vaults = 2;
        plan.key = "k".into();
        plan.roll("2026-10-29".into());
        assert_eq!((plan.cleaned, plan.vaults), (FREE_CLEAN, 2));
        plan.roll("2026-10-30".into());
        assert_eq!(plan, Plan { key: "k".into(), pro_until: 1000, day: "2026-10-30".into(), cleaned: 0, vaults: 0 });
    }

    /// What a buyer sees: a key entered once in the Pro dialog is still Pro after a restart. And the
    /// other direction, which matters more: no missing or damaged file may ever unlock Pro.
    #[test]
    fn a_license_survives_a_restart() {
        let saved = Plan {
            key: "DM-ABCD-EFGH-JKLM".into(),
            pro_until: 2_000_000_000,
            day: "2026-10-30".into(),
            cleaned: 1 << 30,
            vaults: 1,
        };
        assert_eq!(Plan::parse(&saved.text()), saved);
        assert!(Plan::parse(&saved.text()).pro_at(2_000_000_000));

        // Written by a version with fewer fields: the key and the paid period still come back.
        assert_eq!(
            Plan::parse("DM-ABCD-EFGH-JKLM\n2000000000\n"),
            Plan { key: "DM-ABCD-EFGH-JKLM".into(), pro_until: 2_000_000_000, ..Plan::default() }
        );

        for text in ["", "\n\n\n\n", "DM-ABCD-EFGH-JKLM\nforever\n"] {
            assert_eq!(Plan::parse(text).pro_until, 0, "{text:?}");
            assert!(!Plan::parse(text).pro_at(2_000_000_000), "{text:?}");
        }
    }
}
