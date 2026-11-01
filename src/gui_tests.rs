//! Runs the real GUI invisibly and clicks its real buttons: nothing reaches the keyboard or the mouse, and HOME is
//! a throwaway folder, so the user's settings and files are out of reach. Set DESKMONK_TEST_HOME to a folder on a
//! disposable volume to keep the test's files out of the real Trash on macOS.

use super::*;
use egui_kittest::{Harness, kittest::Queryable};
use std::{fs, path::Path, time::{Duration, Instant, SystemTime}};

const SIZE: usize = 40 * 1024; // Above the duplicate finder's minimum.

fn write(home: &Path, name: &str, fill: u8) -> PathBuf {
    let path = home.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, vec![fill; SIZE]).unwrap();
    path
}

fn app_harness<'a>() -> Harness<'a, App> {
    Harness::builder().with_size(vec2(1080.0, 900.0)).with_max_steps(20).build_eframe(|cc| {
        setup_style(&cc.egui_ctx);
        App::default()
    })
}

/// Runs frames until `done`, like the window does while background work goes on.
fn until(h: &mut Harness<App>, what: &str, done: impl Fn(&App) -> bool) {
    let start = Instant::now();
    while !done(h.state()) {
        assert!(start.elapsed() < Duration::from_secs(60), "timed out: {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.step();
    h.step();
}

fn click(h: &mut Harness<App>, label: &str) {
    h.get_by_label(label).click();
    h.step();
    h.step();
}

fn selected_names(app: &App, cat: Category) -> Vec<String> {
    let mut names: Vec<String> = app.items.iter().filter(|i| i.category == cat && i.selected).map(|i| i.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

fn scan(h: &mut Harness<App>) {
    click(h, "🔍  Scan");
    until(h, "scan", |a| a.scanning.is_none() && a.has_scanned);
}

#[test]
fn the_app_works() {
    let home = std::env::var_os("DESKMONK_TEST_HOME").map_or_else(|| std::env::temp_dir().join(format!("deskmonk-gui-{}", std::process::id())), PathBuf::from);
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).unwrap();
    let home = home.canonicalize().unwrap();
    // Before anything reads it. Safe: this binary has no other test, so no other thread runs yet.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("XDG_CACHE_HOME", home.join(".cache"));
        std::env::set_var("XDG_DATA_HOME", home.join(".local/share"));
    }
    i18n::set(Lang::En);
    // Linux reads the user's folders from this file; without it they have no location at all.
    fs::create_dir_all(home.join(".config")).unwrap();
    fs::write(home.join(".config/user-dirs.dirs"), "XDG_DOWNLOAD_DIR=\"$HOME/Downloads\"\nXDG_DOCUMENTS_DIR=\"$HOME/Documents\"\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\n").unwrap();

    // Three duplicate groups: two in the user's folders, one elsewhere (not ticked by itself).
    let photo_kept = write(&home, "Desktop/photo 2.jpg", b'a');
    let photo_b = write(&home, "Documents/photo copy.jpg", b'a');
    let photo_c = write(&home, "Documents/photo.jpg", b'a');
    let report_kept = write(&home, "Documents/report.pdf", b'b');
    let report_b = write(&home, "Downloads/report (1).pdf", b'b');
    let other_kept = write(&home, "Projects/x.bin", b'c');
    let other_b = write(&home, "Projects/y.bin", b'c');
    let notes = write(&home, "Documents/notes.txt", b'd');
    let old = write(&home, "Downloads/old.zip", b'e');
    fs::File::options().write(true).open(&old).unwrap().set_modified(SystemTime::now() - Duration::from_secs(200 * 24 * 3600)).unwrap();
    let movie = home.join("Documents/movie.mov");
    fs::File::create(&movie).unwrap().set_len(deskmonk::scan::LARGE_FILE + 1).unwrap();
    let cache = write(&home, if cfg!(target_os = "macos") { "Library/Caches/app/cache.bin" } else { ".cache/app/cache.bin" }, b'f');

    let mut h = app_harness();
    h.step();
    h.step();

    // The scan finds what is there.
    h.state_mut().root = home.display().to_string();
    scan(&mut h);
    let count = |h: &Harness<App>, cat| h.state().items.iter().filter(|i| i.category == cat).count();
    assert_eq!(count(&h, Category::Duplicate), 7, "all members are listed");
    assert_eq!(h.state().totals(Some(Category::Duplicate), false), (4, 4 * SIZE as u64), "the copies that stay are not counted");
    assert_eq!(selected_names(h.state(), Category::Duplicate), ["photo copy.jpg", "photo.jpg", "report (1).pdf"], "only the user's folders are ticked by themselves");
    assert_eq!(count(&h, Category::OldDownload), 1);
    assert_eq!(count(&h, Category::Large), 1);
    assert_eq!(count(&h, Category::Junk), 1);
    let (found_n, _) = h.state().totals(None, false);
    assert_eq!(found_n, 4 + 3, "Found adds up across the categories");
    assert!(h.query_all_by_value("7 files").count() > 0, "the Found card shows it on screen");

    // Deselect, then Select all, with the real buttons, on the open tab only. (The sidebar has no widget to click.)
    h.state_mut().tab = Category::Duplicate;
    h.step();
    h.step();
    let junk_before = selected_names(h.state(), Category::Junk);
    click(&mut h, "Deselect");
    assert!(selected_names(h.state(), Category::Duplicate).is_empty(), "Deselect");
    click(&mut h, "Select all");
    assert_eq!(selected_names(h.state(), Category::Duplicate), ["photo copy.jpg", "photo.jpg", "report (1).pdf", "y.bin"], "Select all ticks every copy but the first of each group");
    assert_eq!(h.state().totals(Some(Category::Duplicate), true), (4, 4 * SIZE as u64));
    assert_eq!(selected_names(h.state(), Category::Junk), junk_before, "other tabs are left alone");
    // Only the duplicates go this time.
    for cat in [Category::Junk, Category::OldDownload, Category::Large] {
        deskmonk::set_selection(&mut h.state_mut().items, Some(cat), false);
    }
    h.step();
    h.step();
    assert!(h.query_all_by_value("4 files selected").count() > 0, "the screen shows the selection");

    // The footer button, then the question, then the real deletion.
    let label = h.state().totals(None, true).1;
    click(&mut h, &format!("🗑  Clean {}", human_size(label)));
    assert!(h.state().confirm, "Clean asks first");
    click(&mut h, "Cancel");
    assert!(!h.state().confirm);
    assert!(photo_b.exists(), "Cancel deletes nothing");
    click(&mut h, &format!("🗑  Clean {}", human_size(label)));
    click(&mut h, "Clean");
    until(&mut h, "cleaning", |a| a.deleting.is_none());

    for gone in [&photo_b, &photo_c, &report_b, &other_b] {
        assert!(!gone.exists(), "{} should be gone", gone.display());
    }
    for kept in [&photo_kept, &report_kept, &other_kept, &notes, &movie, &old, &cache] {
        assert!(kept.exists(), "{} should stay", kept.display());
    }
    assert!(h.state().status.contains("4 items cleaned"), "status: {}", h.state().status);
    assert!(!h.state().status.contains("could not"), "status: {}", h.state().status);
    assert_eq!(count(&h, Category::Duplicate), 0, "a copy with no others left is no duplicate");
    assert_eq!(h.state().totals(Some(Category::Duplicate), false), (0, 0));

    // A file that can't be deleted is reported, stays ticked and leaves the rest alone.
    {
        let app = h.state_mut();
        app.items.push(Item { path: home.join("Documents/never existed"), size: 7, category: Category::Large, group: 0, selected: true });
        deskmonk::set_selection(&mut app.items, Some(Category::OldDownload), true);
        for i in app.items.iter_mut().filter(|i| i.path == movie) { i.selected = false; }
    }
    h.step();
    let label = h.state().totals(None, true).1;
    click(&mut h, &format!("🗑  Clean {}", human_size(label)));
    click(&mut h, "Clean");
    until(&mut h, "cleaning with a failure", |a| a.deleting.is_none());
    assert!(!old.exists(), "the good one still goes");
    assert!(h.state().status.contains("1 items cleaned") && h.state().status.contains("1 items could not"), "status: {}", h.state().status);
    assert!(h.state().items.iter().any(|i| i.selected && i.path.ends_with("never existed")), "the failed one stays ticked");

    // The next cleaning starts without the old message; here the failed one is gone from the list first.
    h.state_mut().items.retain(|i| !i.path.ends_with("never existed"));
    h.state_mut().items.iter_mut().filter(|i| i.category == Category::Junk).for_each(|i| i.selected = true);
    h.step();
    let label = h.state().totals(None, true).1;
    click(&mut h, &format!("🗑  Clean {}", human_size(label)));
    click(&mut h, "Clean");
    until(&mut h, "second cleaning", |a| a.deleting.is_none());
    assert!(!h.state().status.contains("could not"), "the old failure is not shown again: {}", h.state().status);
    assert!(!cache.exists(), "junk is erased");

    // A new scan agrees with what is left on disk.
    scan(&mut h);
    assert_eq!(count(&h, Category::Duplicate), 0, "no duplicates left after cleaning them");
    assert_eq!(count(&h, Category::Large), 1);

    // Disk map: measures the folder for real.
    h.state_mut().page = Page::Map;
    h.state_mut().root = home.join("Documents").display().to_string();
    h.step();
    h.step();
    click(&mut h, "▶ Map");
    until(&mut h, "disk map", |a| a.map_scan.is_none() && a.map.is_some());
    assert!(!h.state().map.as_ref().unwrap().children.is_empty(), "the map lists what is in the folder");

    // Hardware: reads this computer.
    h.state_mut().page = Page::Hardware;
    let ctx = h.ctx.clone();
    h.state_mut().detect_hardware(ctx);
    until(&mut h, "hardware", |a| a.hardware.is_some());

    // Monk mode: choose a length, start, reset.
    h.state_mut().page = Page::Monk;
    h.step();
    h.step();
    assert!(h.state().monk.phase == focus::Phase::Idle);
    click(&mut h, "45");
    click(&mut h, "▶  Start focusing");
    assert!(h.state().monk.phase != focus::Phase::Idle, "the timer runs");
    click(&mut h, "Reset");
    assert!(h.state().monk.phase == focus::Phase::Idle, "Reset stops it");

    // Vault: a bad password is refused, then encrypt and open again, byte for byte.
    let secret = home.join("Documents/secret.txt");
    let content: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    fs::write(&secret, &content).unwrap();
    h.state_mut().page = Page::Vault;
    h.state_mut().vault_pick(secret.clone());
    h.step();
    h.step();
    h.state_mut().vault_pw = "abc".into();
    h.state_mut().vault_pw2 = "abc".into();
    h.step();
    click(&mut h, "Encrypt");
    assert!(h.state().vault_error.contains("at least 4"), "too short: {}", h.state().vault_error);
    h.state_mut().vault_pw = "correct horse".into();
    h.state_mut().vault_pw2 = "something else".into();
    h.step();
    click(&mut h, "Encrypt");
    assert!(h.state().vault_error.contains("do not match"), "mismatch: {}", h.state().vault_error);
    assert!(secret.exists(), "nothing happens to the file until the passwords are right");
    h.state_mut().vault_pw2 = "correct horse".into();
    h.step();
    click(&mut h, "Encrypt");
    until(&mut h, "encrypting", |a| a.vault_job.is_none() && a.vault_done.is_some());
    let vault_file = h.state().vault_done.as_ref().unwrap().0.clone();
    assert!(deskmonk::vault::is_vault(&vault_file), "{} is a vault", vault_file.display());
    assert!(!secret.exists(), "the readable copy is gone");
    let on_disk = fs::read(&vault_file).unwrap();
    assert!(!on_disk.windows(64).any(|w| content.windows(64).next() == Some(w)), "the vault doesn't contain the text");

    h.state_mut().vault_pick(vault_file.clone());
    h.step();
    h.step();
    h.state_mut().vault_pw = "wrong password".into();
    h.step();
    click(&mut h, "Open vault");
    until(&mut h, "wrong password", |a| a.vault_job.is_none());
    assert!(!h.state().vault_error.is_empty(), "a wrong password is refused");
    assert!(vault_file.exists() && !secret.exists(), "and changes nothing");
    h.state_mut().vault_pw = "correct horse".into();
    h.step();
    click(&mut h, "Open vault");
    until(&mut h, "opening", |a| a.vault_job.is_none() && a.vault_done.is_some());
    let opened = h.state().vault_done.as_ref().unwrap().0.clone();
    assert_eq!(fs::read(&opened).unwrap(), content, "the opened file is the same, byte for byte");
    h.state_mut().vault_target = None;
    h.state_mut().vault_done = None;

    // Every page opens and draws, in every language.
    for page in [Page::Monk, Page::Hardware, Page::Map, Page::Lock, Page::Vault, Page::License, Page::Clean] {
        h.state_mut().page = page;
        for lang in [Lang::Tr, Lang::Es, Lang::En] {
            i18n::set(lang);
            h.step();
            h.step();
        }
    }
    let _ = fs::remove_dir_all(&home);
}
