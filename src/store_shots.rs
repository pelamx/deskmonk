//! Microsoft Store screenshots, drawn by the real GUI on Windows: `cargo test store_shots -- --ignored` writes
//! `dist/store-shots/<lang>-<n>-<page>.png` at 2732×1536 (add `--release`: a debug build scans slowly). Ignored by default: on Windows the settings ignore HOME,
//! so it saves the language and scans a sample folder in the real profile (CI's throwaway one, in the msix job).

use super::*;
use egui_kittest::{Harness, kittest::Queryable};
use std::{fs, time::{Duration, Instant}};

fn until(h: &mut Harness<App>, what: &str, done: impl Fn(&App) -> bool) {
    let start = Instant::now();
    while !done(h.state()) {
        assert!(start.elapsed() < Duration::from_secs(900), "timed out: {what} (status: {}, phase: {:?})", h.state().status, h.state().phase);
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.step();
    h.step();
}

#[test]
#[ignore]
fn store_shots() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("dist/store-shots");
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();

    // Something worth showing on the Clean page: copies, an old download, a big video.
    // Not in %TEMP%: the scan reads all of it as junk, and a CI runner's is large.
    let home = PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE")).join("deskMonk sample");
    let _ = fs::remove_dir_all(&home);
    for (name, fill, len) in [
        ("Pictures/holiday.jpg", 1u8, 3_400_000u64),
        ("Downloads/holiday (1).jpg", 1, 3_400_000),
        ("Documents/report.pdf", 2, 820_000),
        ("Downloads/report (1).pdf", 2, 820_000),
        ("Desktop/report copy.pdf", 2, 820_000),
        ("Downloads/setup-old.exe", 3, 46_000_000),
        ("Videos/screen recording.mp4", 4, 0),
    ] {
        let path = home.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, vec![fill; if len == 0 { 1024 } else { len as usize }]).unwrap();
        if len == 0 {
            fs::File::options().write(true).open(&path).unwrap().set_len(deskmonk::scan::LARGE_FILE + 200_000_000).unwrap();
        }
    }
    let old = home.join("Downloads/setup-old.exe");
    fs::File::options().write(true).open(&old).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(300 * 86400)).unwrap();

    let mut h = Harness::builder().with_size(vec2(1366.0, 768.0)).with_pixels_per_point(2.0).with_max_steps(20).wgpu().build_eframe(|cc| {
        setup_style(&cc.egui_ctx);
        App::default()
    });
    h.step();
    h.state_mut().root = home.display().to_string();
    h.step();
    h.get_by_label("🔍  Scan").click();
    h.step();
    until(&mut h, "scan", |a| a.scanning.is_none() && a.has_scanned);

    h.state_mut().page = Page::Map;
    h.state_mut().root = env!("CARGO_MANIFEST_DIR").to_string();
    h.step();
    h.get_by_label("▶ Map").click();
    h.step();
    until(&mut h, "disk map", |a| a.map_scan.is_none() && a.map.is_some());

    let ctx = h.ctx.clone();
    h.state_mut().detect_hardware(ctx);
    until(&mut h, "hardware", |a| a.hardware.is_some());

    // App lock is macOS and Linux only, so it has no shot.
    for (lang, code) in [(Lang::En, "en"), (Lang::Tr, "tr"), (Lang::Es, "es")] {
        i18n::set(lang);
        for (n, (page, name)) in [(Page::Clean, "clean"), (Page::Vault, "vault"), (Page::Monk, "monk"), (Page::Map, "map"), (Page::Hardware, "hardware")]
            .into_iter()
            .enumerate()
        {
            h.state_mut().page = page;
            h.state_mut().root = if name == "map" { env!("CARGO_MANIFEST_DIR").to_string() } else { home.display().to_string() };
            for _ in 0..4 {
                h.step();
            }
            let image = h.render().expect("render");
            image.save(out.join(format!("{code}-{}-{name}.png", n + 1))).unwrap();
        }
    }
    let _ = fs::remove_dir_all(&home);
}
