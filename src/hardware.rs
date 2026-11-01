//! Hardware inventory with component brands and live usage, read with the OS's own tools.
//! Never collects serial numbers, host names or user names: these specs are sent to the admin panel.

use crate::i18n::t;
use crate::scan::human_size;
use crate::tf;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    /// Fixed Turkish key, e.g. "İşlemci": the stats server queries it. Show it with [`kind_label`].
    pub kind: &'static str,
    /// Brand name, empty when unknown.
    pub brand: String,
    pub name: String,
    /// Specs and usage (free space, battery health…).
    pub detail: String,
}

/// (whole word or phrase to look for in lowercase, brand, brand color readable on a dark background).
/// Hex codes are JEDEC memory maker ids that Windows reports instead of names.
const BRANDS: &[(&str, &str, [u8; 3])] = &[
    ("apple", "Apple", [235, 235, 240]),
    ("intel", "Intel", [0, 174, 239]),
    ("amd", "AMD", [237, 28, 36]),
    ("ryzen", "AMD", [237, 28, 36]),
    ("radeon", "AMD", [237, 28, 36]),
    ("nvidia", "NVIDIA", [118, 185, 0]),
    ("geforce", "NVIDIA", [118, 185, 0]),
    ("qualcomm", "Qualcomm", [50, 100, 255]),
    ("snapdragon", "Qualcomm", [50, 100, 255]),
    ("samsung", "Samsung", [80, 130, 255]),
    ("80ce", "Samsung", [80, 130, 255]),
    ("hynix", "SK hynix", [255, 60, 90]),
    ("80ad", "SK hynix", [255, 60, 90]),
    ("micron", "Micron", [0, 160, 220]),
    ("802c", "Micron", [0, 160, 220]),
    ("crucial", "Crucial", [0, 160, 220]),
    ("kingston", "Kingston", [230, 40, 50]),
    ("0198", "Kingston", [230, 40, 50]),
    ("corsair", "Corsair", [255, 210, 0]),
    ("g skill", "G.Skill", [220, 30, 40]),
    ("western digital", "Western Digital", [0, 150, 255]),
    ("wdc", "Western Digital", [0, 150, 255]),
    ("seagate", "Seagate", [110, 200, 70]),
    ("toshiba", "Toshiba", [255, 40, 40]),
    ("kioxia", "Kioxia", [255, 255, 255]),
    ("sandisk", "SanDisk", [230, 30, 40]),
    ("adata", "ADATA", [255, 60, 60]),
    ("lenovo", "Lenovo", [230, 30, 40]),
    ("dell", "Dell", [0, 140, 255]),
    ("hp", "HP", [0, 150, 214]),
    ("hewlett packard", "HP", [0, 150, 214]),
    ("asus", "ASUS", [0, 170, 255]),
    ("asustek", "ASUS", [0, 170, 255]),
    ("acer", "Acer", [131, 184, 26]),
    ("msi", "MSI", [255, 0, 0]),
    ("micro star", "MSI", [255, 0, 0]),
    ("gigabyte", "Gigabyte", [255, 120, 0]),
    ("microsoft", "Microsoft", [0, 164, 239]),
    ("huawei", "Huawei", [255, 50, 50]),
    ("xiaomi", "Xiaomi", [255, 105, 0]),
    ("razer", "Razer", [68, 214, 44]),
    ("lg", "LG", [255, 40, 90]),
    ("benq", "BenQ", [140, 100, 255]),
    ("sony", "Sony", [235, 235, 240]),
    ("realtek", "Realtek", [70, 130, 255]),
    ("vmware", "VMware", [120, 190, 30]),
];

/// Brand mentioned in `text` as a whole word ("Intel(R) Core" yes, "chipset" is not HP).
pub fn brand_of(text: &str) -> Option<(&'static str, [u8; 3])> {
    let words: String = text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let words = format!(" {words} ");
    BRANDS
        .iter()
        .find(|(needle, ..)| words.contains(&format!(" {needle} ")))
        .map(|&(_, brand, color)| (brand, color))
}

/// `hint` is a vendor field the OS reports (may be empty); the brand is also looked for in the name.
fn part(kind: &'static str, hint: &str, name: impl Into<String>, detail: impl Into<String>) -> Component {
    let name = name.into();
    let brand = brand_of(&format!("{hint} {name}"))
        .map(|(b, _)| b.to_string())
        .unwrap_or_else(|| hint.trim().to_string());
    Component { kind, brand, name: name.trim().to_string(), detail: detail.into() }
}

/// A component kind in the UI language.
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "Bilgisayar" => t("Bilgisayar", "Computer", "Equipo"),
        "Sistem" => t("Sistem", "System", "Sistema"),
        "İşlemci" => t("İşlemci", "Processor", "Procesador"),
        "Bellek" => t("Bellek", "Memory", "Memoria"),
        "Ekran kartı" => t("Ekran kartı", "Graphics", "Gráficos"),
        "Ekran" => t("Ekran", "Display", "Pantalla"),
        "Depolama" => t("Depolama", "Storage", "Almacenamiento"),
        "Disk alanı" => t("Disk alanı", "Disk space", "Espacio en disco"),
        "Pil" => t("Pil", "Battery", "Batería"),
        _ => t("Diğer", "Other", "Otro"),
    }
}

/// "600 GB used / 900 GB · 300 GB free".
fn used_free(used: u64, size: u64, free: u64) -> String {
    let (used, size, free) = (human_size(used), human_size(size), human_size(free));
    tf!("{used} dolu / {size} · {free} boş", "{used} used / {size} · {free} free", "{used} usado / {size} · {free} libre")
}

fn in_use(used: u64, total: u64) -> String {
    let (used, total) = (human_size(used), human_size(total));
    tf!("{used} kullanımda / {total}", "{used} in use / {total}", "{used} en uso / {total}")
}

/// Slow (spawns system tools, up to a few seconds on Windows): call it off the UI thread.
pub fn detect() -> Vec<Component> {
    #[cfg(target_os = "macos")]
    let parts = parse_mac(&json(run(
        "system_profiler",
        &[
            "SPHardwareDataType", "SPSoftwareDataType", "SPDisplaysDataType", "SPStorageDataType",
            "SPMemoryDataType", "SPPowerDataType", "-json", "-detailLevel", "mini",
        ],
    )));
    #[cfg(target_os = "windows")]
    let parts = parse_windows(&json(run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", WINDOWS_QUERY],
    )));
    #[cfg(target_os = "linux")]
    let parts = linux();
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let parts = Vec::new();
    parts
}

#[allow(dead_code)]
fn run(cmd: &str, args: &[&str]) -> String {
    let mut command = std::process::Command::new(cmd);
    command.args(args);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flashing over the GUI
    }
    command
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

#[allow(dead_code)]
fn json(text: String) -> Value {
    serde_json::from_str(&text).unwrap_or_default()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// A JSON list; Windows PowerShell 5 sometimes wraps lists as {"value": [...]} or sends a lone object.
fn list(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) if o.get("value").is_some_and(Value::is_array) => list(&o["value"]),
        Value::Null => Vec::new(),
        other => vec![other],
    }
}

fn num(v: &Value) -> u64 {
    v.as_u64().or_else(|| s(v).parse().ok()).unwrap_or(0)
}

#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn parse_mac(v: &Value) -> Vec<Component> {
    let mut parts = Vec::new();
    let hw = &v["SPHardwareDataType"][0];
    if hw.is_object() {
        parts.push(part(
            "Bilgisayar",
            "Apple",
            s(&hw["machine_name"]),
            format!("{} · {}", s(&hw["machine_model"]), s(&hw["model_number"])),
        ));
    }
    let sw = &v["SPSoftwareDataType"][0];
    if sw.is_object() {
        let uptime = s(&sw["uptime"]).trim_start_matches("up ").split(':').collect::<Vec<_>>();
        let uptime = match uptime[..] {
            [d, h, m, _] => tf!("çalışma süresi {d} gün {h} sa {m} dk", "up {d} d {h} h {m} min", "encendido {d} d {h} h {m} min"),
            _ => String::new(),
        };
        parts.push(part("Sistem", "Apple", s(&sw["os_version"]), uptime));
    }
    // Apple silicon reports the chip, Intel Macs the CPU.
    let cpu = hw["chip_type"].as_str().or(hw["cpu_type"].as_str()).unwrap_or("");
    if !cpu.is_empty() {
        let cores = match s(&hw["number_processors"]).strip_prefix("proc ") {
            Some(split) => match split.split(':').collect::<Vec<_>>()[..] {
                [all, perf, eff, ..] => tf!(
                    "{all} çekirdek ({perf} performans + {eff} verimlilik)",
                    "{all} cores ({perf} performance + {eff} efficiency)",
                    "{all} núcleos ({perf} de rendimiento + {eff} de eficiencia)"
                ),
                _ => String::new(),
            },
            None => {
                let (n, speed) = (num(&hw["number_processors"]), s(&hw["current_processor_speed"]));
                tf!("{n} çekirdek · {speed}", "{n} cores · {speed}", "{n} núcleos · {speed}")
            }
        };
        parts.push(part("İşlemci", "", cpu, cores));
    }
    let mem = &v["SPMemoryDataType"][0];
    let maker = mem["dimm_manufacturer"].as_str().or(mem["_items"][0]["dimm_manufacturer"].as_str()).unwrap_or("");
    let kind = mem["dimm_type"].as_str().or(mem["_items"][0]["dimm_type"].as_str()).unwrap_or("");
    if hw.is_object() {
        parts.push(part("Bellek", maker, format!("{} {kind}", s(&hw["physical_memory"])), ""));
    }
    for gpu in list(&v["SPDisplaysDataType"]) {
        let vendor = s(&gpu["spdisplays_vendor"]).trim_start_matches("sppci_vendor_");
        let detail = match (s(&gpu["sppci_cores"]), gpu["spdisplays_vram"].as_str().or(gpu["spdisplays_vram_shared"].as_str())) {
            ("", Some(vram)) => format!("{vram} VRAM"),
            ("", None) => String::new(),
            (cores, _) => tf!("{cores} GPU çekirdeği", "{cores} GPU cores", "{cores} núcleos de GPU"),
        };
        parts.push(part("Ekran kartı", vendor, s(&gpu["_name"]), detail));
        for screen in list(&gpu["spdisplays_ndrvs"]) {
            // 610 is Apple's display vendor id; built-in panels are just called "Color LCD".
            let hint = if s(&screen["_spdisplays_display-vendor-id"]) == "610" { "Apple" } else { "" };
            let panel = s(&screen["spdisplays_display_type"])
                .trim_start_matches("spdisplays_")
                .replace('-', " ");
            parts.push(part(
                "Ekran",
                hint,
                s(&screen["_name"]),
                format!("{} {panel}", s(&screen["_spdisplays_resolution"])).trim().to_string(),
            ));
        }
    }
    // One drive has several volumes sharing its space: show each drive once, with its biggest volume.
    let mut drives: Vec<(&str, &Value)> = Vec::new();
    for vol in list(&v["SPStorageDataType"]) {
        let name = s(&vol["physical_drive"]["device_name"]);
        // Mounted .dmg files are listed as drives too.
        if name.is_empty() || s(&vol["physical_drive"]["protocol"]) == "Disk Image" {
            continue;
        }
        match drives.iter_mut().find(|(n, _)| *n == name) {
            Some(d) if num(&vol["size_in_bytes"]) > num(&d.1["size_in_bytes"]) => d.1 = vol,
            Some(_) => {}
            None => drives.push((name, vol)),
        }
    }
    for (name, vol) in drives {
        let (size, free) = (num(&vol["size_in_bytes"]), num(&vol["free_space_in_bytes"]));
        let drive = &vol["physical_drive"];
        parts.push(part(
            "Depolama",
            "",
            name,
            format!("{} · {} {}", used_free(size.saturating_sub(free), size, free), s(&drive["medium_type"]).to_uppercase(), s(&drive["protocol"])),
        ));
    }
    for power in list(&v["SPPowerDataType"]) {
        if s(&power["_name"]) != "spbattery_information" {
            continue;
        }
        let health = &power["sppower_battery_health_info"];
        parts.push(part(
            "Pil",
            "",
            t("Dahili pil", "Internal battery", "Batería interna"),
            {
                let charge = num(&power["sppower_battery_charge_info"]["sppower_battery_state_of_charge"]);
                let state = s(&health["sppower_battery_health"]);
                let max = s(&health["sppower_battery_health_maximum_capacity"]).replace('\u{a0}', "");
                let cycles = num(&health["sppower_battery_cycle_count"]);
                tf!(
                    "%{charge} şarj · sağlık {state} ({max}) · {cycles} döngü",
                    "{charge}% charged · health {state} ({max}) · {cycles} cycles",
                    "{charge}% de carga · salud {state} ({max}) · {cycles} ciclos"
                )
            },
        ));
    }
    parts
}

#[cfg(target_os = "windows")]
const WINDOWS_QUERY: &str = "$ErrorActionPreference='SilentlyContinue'; @{
cs=Get-CimInstance Win32_ComputerSystem | Select-Object Manufacturer,Model,TotalPhysicalMemory
os=Get-CimInstance Win32_OperatingSystem | Select-Object Caption,FreePhysicalMemory
cpu=@(Get-CimInstance Win32_Processor | Select-Object Name,Manufacturer,NumberOfCores,NumberOfLogicalProcessors)
gpu=@(Get-CimInstance Win32_VideoController | Select-Object Name,AdapterCompatibility,CurrentHorizontalResolution,CurrentVerticalResolution)
ram=@(Get-CimInstance Win32_PhysicalMemory | Select-Object Manufacturer,PartNumber,Capacity,Speed)
disk=@(Get-CimInstance Win32_DiskDrive | Select-Object Model,Size)
vol=@(Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' | Select-Object DeviceID,Size,FreeSpace)
bat=@(Get-CimInstance Win32_Battery | Select-Object Name,EstimatedChargeRemaining)
} | ConvertTo-Json -Depth 3 -Compress";

#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn parse_windows(v: &Value) -> Vec<Component> {
    let mut parts = Vec::new();
    let cs = &v["cs"];
    if cs.is_object() {
        let maker = s(&cs["Manufacturer"]);
        parts.push(part("Bilgisayar", maker, format!("{maker} {}", s(&cs["Model"])), ""));
    }
    if v["os"].is_object() {
        parts.push(part("Sistem", "Microsoft", s(&v["os"]["Caption"]), ""));
    }
    for cpu in list(&v["cpu"]) {
        let (cores, threads) = (num(&cpu["NumberOfCores"]), num(&cpu["NumberOfLogicalProcessors"]));
        let detail = tf!("{cores} çekirdek / {threads} iş parçacığı", "{cores} cores / {threads} threads", "{cores} núcleos / {threads} hilos");
        parts.push(part("İşlemci", s(&cpu["Manufacturer"]), s(&cpu["Name"]), detail));
    }
    let total = num(&cs["TotalPhysicalMemory"]);
    let free = num(&v["os"]["FreePhysicalMemory"]) * 1024;
    let mut ram_detail = in_use(total.saturating_sub(free), total);
    let sticks = list(&v["ram"]);
    if let Some(stick) = sticks.first() {
        let (n, speed, part_no) = (sticks.len(), num(&stick["Speed"]), s(&stick["PartNumber"]).trim());
        ram_detail += &tf!(" · {n} modül, {speed} MHz {part_no}", " · {n} modules, {speed} MHz {part_no}", " · {n} módulos, {speed} MHz {part_no}");
    }
    let maker = sticks.first().map_or("", |m| s(&m["Manufacturer"]));
    parts.push(part("Bellek", maker, human_size(total), ram_detail));
    for gpu in list(&v["gpu"]) {
        let res = match (num(&gpu["CurrentHorizontalResolution"]), num(&gpu["CurrentVerticalResolution"])) {
            (0, _) | (_, 0) => String::new(),
            (w, h) => format!("{w} x {h}"),
        };
        parts.push(part("Ekran kartı", s(&gpu["AdapterCompatibility"]), s(&gpu["Name"]), res));
    }
    for disk in list(&v["disk"]) {
        parts.push(part("Depolama", "", s(&disk["Model"]), human_size(num(&disk["Size"]))));
    }
    for vol in list(&v["vol"]) {
        let (size, free) = (num(&vol["Size"]), num(&vol["FreeSpace"]));
        parts.push(part("Disk alanı", "", s(&vol["DeviceID"]), used_free(size.saturating_sub(free), size, free)));
    }
    for bat in list(&v["bat"]) {
        let charge = num(&bat["EstimatedChargeRemaining"]);
        parts.push(part("Pil", "", s(&bat["Name"]), tf!("%{charge} şarj", "{charge}% charged", "{charge}% de carga")));
    }
    parts
}

#[cfg(target_os = "linux")]
fn linux() -> Vec<Component> {
    use std::fs;
    let read = |p: &str| fs::read_to_string(p).unwrap_or_default().trim().to_string();
    let mut parts = Vec::new();
    let vendor = read("/sys/class/dmi/id/sys_vendor");
    parts.push(part("Bilgisayar", &vendor, format!("{vendor} {}", read("/sys/class/dmi/id/product_name")), ""));
    let os = read("/etc/os-release");
    let os = os.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).unwrap_or("Linux").trim_matches('"');
    let up = read("/proc/uptime").split('.').next().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
    let (d, h, m) = (up / 86400, up / 3600 % 24, up / 60 % 60);
    let uptime = tf!("çalışma süresi {d} gün {h} sa {m} dk", "up {d} d {h} h {m} min", "encendido {d} d {h} h {m} min");
    parts.push(part("Sistem", "", os, uptime));
    let cpuinfo = read("/proc/cpuinfo");
    let field = |key: &str| cpuinfo.lines().find(|l| l.starts_with(key)).and_then(|l| l.split(':').nth(1)).unwrap_or("").trim();
    let threads = cpuinfo.lines().filter(|l| l.starts_with("processor")).count();
    parts.push(part("İşlemci", field("vendor_id"), field("model name"), tf!("{threads} iş parçacığı", "{threads} threads", "{threads} hilos")));
    let meminfo = read("/proc/meminfo");
    let kb = |key: &str| meminfo.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0) * 1024;
    let (total, avail) = (kb("MemTotal:"), kb("MemAvailable:"));
    parts.push(part("Bellek", "", human_size(total), in_use(total.saturating_sub(avail), total)));
    // `lspci -mm` lines: slot "class" "vendor" "device" ...
    for line in run("lspci", &["-mm"]).lines().filter(|l| l.contains("\"VGA") || l.contains("\"3D")) {
        let fields: Vec<&str> = line.split('"').collect();
        if let [_, _, _, vendor, _, device, ..] = fields[..] {
            parts.push(part("Ekran kartı", vendor, device, ""));
        }
    }
    for entry in fs::read_dir("/sys/block").into_iter().flatten().flatten() {
        let dev = entry.file_name().to_string_lossy().into_owned();
        if ["loop", "ram", "zram", "dm-", "sr"].iter().any(|p| dev.starts_with(p)) {
            continue;
        }
        let base = format!("/sys/block/{dev}");
        let size = read(&format!("{base}/size")).parse::<u64>().unwrap_or(0) * 512;
        let vendor = read(&format!("{base}/device/vendor"));
        let model = read(&format!("{base}/device/model"));
        let ssd = if read(&format!("{base}/queue/rotational")) == "0" { "SSD" } else { "HDD" };
        parts.push(part("Depolama", &vendor, if model.is_empty() { dev } else { model }, format!("{} · {ssd}", human_size(size))));
    }
    // `df -kP /`: second line is "filesystem blocks used available ..."; sizes in KB.
    let df = run("df", &["-kP", "/"]);
    if let Some([size, used, free]) = df.lines().nth(1).map(|l| l.split_whitespace().skip(1).take(3).filter_map(|n| n.parse::<u64>().ok()).map(|n| n * 1024).collect::<Vec<_>>()).and_then(|v| <[u64; 3]>::try_from(v).ok()) {
        parts.push(part("Disk alanı", "", "/", used_free(used, size, free)));
    }
    for entry in fs::read_dir("/sys/class/power_supply").into_iter().flatten().flatten() {
        let base = entry.path().display().to_string();
        if read(&format!("{base}/type")) != "Battery" {
            continue;
        }
        // Batteries report either energy_* or charge_* files.
        let unit = if read(&format!("{base}/energy_full")).is_empty() { "charge" } else { "energy" };
        let number = |name: &str| read(&format!("{base}/{unit}_{name}")).parse::<f64>().ok();
        let (now, full, design) = (number("now"), number("full"), number("full_design"));
        let health = match percent(full, design) {
            Some(pct) => tf!(" · sağlık %{pct:.0}", " · health {pct:.0}%", " · salud {pct:.0}%"),
            None => String::new(),
        };
        let cycles = match read(&format!("{base}/cycle_count")).parse::<u32>() {
            Ok(n) if n > 0 => tf!(" · {n} döngü", " · {n} cycles", " · {n} ciclos"),
            _ => String::new(),
        };
        parts.push(part(
            "Pil",
            &read(&format!("{base}/manufacturer")),
            read(&format!("{base}/model_name")),
            {
                let charge = percent(now, full).or_else(|| percent(read(&format!("{base}/capacity")).parse().ok(), Some(100.0))).unwrap_or(0.0);
                tf!("%{charge:.0} şarj{health}{cycles}", "{charge:.0}% charged{health}{cycles}", "{charge:.0}% de carga{health}{cycles}")
            },
        ));
    }
    parts
}

/// `part` as a share of `whole`, at most 100: a battery can report more than its design capacity (104% on a new pack).
#[cfg(target_os = "linux")]
fn percent(part: Option<f64>, whole: Option<f64>) -> Option<f64> {
    match (part, whole) {
        (Some(part), Some(whole)) if whole > 0.0 => Some((part / whole * 100.0).min(100.0)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn battery_percentages_stop_at_100() {
        // A real pack: full, and holding more than its design capacity.
        assert_eq!(percent(Some(6_652_000.0), Some(6_652_000.0)), Some(100.0));
        assert_eq!(percent(Some(6_652_000.0), Some(6_400_000.0)), Some(100.0));
        assert_eq!(percent(Some(3_326_000.0), Some(6_652_000.0)), Some(50.0));
        assert_eq!(percent(Some(1.0), Some(0.0)), None);
        assert_eq!(percent(None, Some(1.0)), None);
    }

    #[test]
    fn finds_brands_as_whole_words() {
        assert_eq!(brand_of("Intel(R) Core(TM) i7-1165G7").unwrap().0, "Intel");
        assert_eq!(brand_of("NVIDIA GeForce RTX 4070").unwrap().0, "NVIDIA");
        assert_eq!(brand_of("WDC WD10EZEX-08WN4A0").unwrap().0, "Western Digital");
        assert_eq!(brand_of("80CE").unwrap().0, "Samsung");
        assert!(brand_of("Intel chipset").is_some_and(|b| b.0 == "Intel"));
        assert!(brand_of("chipset").is_none());
    }

    #[test]
    fn parses_mac_and_windows_output() {
        let mac = serde_json::json!({
            "SPHardwareDataType": [{"chip_type": "Apple M5 Pro", "machine_name": "MacBook Pro", "machine_model": "Mac17,9",
                "model_number": "X", "number_processors": "proc 15:5:10:0", "physical_memory": "24 GB", "serial_number": "SECRET"}],
            "SPMemoryDataType": [{"dimm_manufacturer": "Samsung", "dimm_type": "LPDDR5"}],
            "SPStorageDataType": [
                {"size_in_bytes": 100, "free_space_in_bytes": 40, "physical_drive": {"device_name": "APPLE SSD AP1024Z"}},
                {"size_in_bytes": 900, "free_space_in_bytes": 300, "physical_drive": {"device_name": "APPLE SSD AP1024Z"}}],
        });
        let parts = parse_mac(&mac);
        let get = |kind| parts.iter().find(|p| p.kind == kind).unwrap();
        assert_eq!(get("İşlemci").detail, "15 cores (5 performance + 10 efficiency)");
        assert_eq!(get("Bellek").brand, "Samsung");
        assert_eq!(parts.iter().filter(|p| p.kind == "Depolama").count(), 1);
        assert!(get("Depolama").detail.starts_with("600 B used / 900 B"));
        assert!(!format!("{parts:?}").contains("SECRET"));

        // PowerShell 5 list wrapping and lone objects.
        let win = serde_json::json!({
            "cpu": {"value": [{"Name": "AMD Ryzen 7 5800X 8-Core Processor", "Manufacturer": "AuthenticAMD", "NumberOfCores": 8}]},
            "ram": {"Manufacturer": "80AD", "Speed": 3200},
            "cs": {"Manufacturer": "ASUSTeK COMPUTER INC.", "Model": "ROG", "TotalPhysicalMemory": 2048},
        });
        let parts = parse_windows(&win);
        let brand = |kind| parts.iter().find(|p| p.kind == kind).unwrap().brand.clone();
        assert_eq!((brand("İşlemci"), brand("Bellek"), brand("Bilgisayar")), ("AMD".into(), "SK hynix".into(), "ASUS".into()));
    }
}
