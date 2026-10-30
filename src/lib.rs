pub mod applock;
pub mod diskmap;
pub mod focus;
pub mod hardware;
pub mod plan;
pub mod i18n;
pub mod scan;
pub mod speedtest;
pub mod telemetry;
pub mod vault;

use scan::Item;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};

/// (total, free) bytes of the disk holding `dir`. Free is what an app can write, so on macOS it leaves out
/// purgeable space that Finder counts as available.
#[cfg(unix)]
pub fn disk_space(dir: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(path.as_ptr(), &mut stat) } == 0)
        .then(|| (stat.f_blocks as u64 * stat.f_frsize as u64, stat.f_bavail as u64 * stat.f_frsize as u64))
}

// shortcut: Windows has no statvfs; GetDiskFreeSpaceExW (windows-sys) would fill this in when needed.
#[cfg(not(unix))]
pub fn disk_space(_: &Path) -> Option<(u64, u64)> {
    None
}

/// A deletion running on a background thread, so the UI never freezes on big folders.
pub struct Deletion {
    pub total: usize,
    pub done: Arc<AtomicUsize>,
    rx: Receiver<Finished>,
}

/// Result of a [`Deletion`].
pub struct Finished {
    /// Items that could not be deleted (completely); they stay selected.
    pub failed: Vec<Item>,
    pub freed: u64,
    /// Reason of the first failure, for the status line.
    pub first_error: Option<String>,
}

/// Whether deleting this item removes it for good. Junk (caches, the trash, temp files) is removed for good:
/// moving it to the trash frees no space and caches regenerate anyway. The user's own files go to the trash.
pub fn is_permanent(item: &Item, permanent: bool) -> bool {
    permanent || item.category == scan::Category::Junk || scan::is_in_trash(&item.path)
}

impl Deletion {
    /// Takes the selected items out of `items` and deletes them in the background, see [`is_permanent`].
    /// `permanent` removes everything for good (needed on servers, where trash frees no space).
    /// `on_done` runs when finished.
    pub fn start(
        items: &mut Vec<Item>,
        permanent: bool,
        on_done: impl FnOnce() + Send + 'static,
    ) -> Self {
        let (selected, rest): (Vec<Item>, Vec<Item>) =
            std::mem::take(items).into_iter().partition(|i| i.selected);
        *items = rest;
        let done = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel();
        let (total, counter) = (selected.len(), done.clone());
        std::thread::spawn(move || {
            let mut result = Finished {
                failed: Vec::new(),
                freed: 0,
                first_error: None,
            };
            for item in selected {
                let outcome = if is_permanent(&item, permanent) {
                    remove(&item.path)
                } else {
                    to_trash(&item.path).map_err(|e| std::io::Error::other(e.to_string()))
                };
                match outcome {
                    Ok(()) => result.freed += item.size,
                    Err(e) => {
                        result
                            .first_error
                            .get_or_insert_with(|| format!("{}: {e}", item.path.display()));
                        result.failed.push(item);
                    }
                }
                counter.fetch_add(1, Ordering::Relaxed);
            }
            let _ = tx.send(result);
            on_done();
        });
        Self { total, done, rx }
    }

    /// The result, once finished.
    pub fn poll(&self) -> Option<Finished> {
        self.rx.try_recv().ok()
    }
}

/// Removes a file or folder tree for good, best effort: read-only folders (left by package builds, Go, …) are
/// made writable first, and a failing entry does not stop the rest of the tree from being freed.
/// The OS trash folder itself is emptied but kept, since the OS expects it to exist.
fn remove(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return fs::remove_file(path).or_else(|e| {
            // Windows refuses to delete read-only files; on Unix the parent folder's permission is what counts.
            if cfg!(windows) && make_writable(path, &meta).is_ok() {
                fs::remove_file(path)
            } else {
                Err(e)
            }
        });
    }
    let _ = make_writable(path, &meta);
    let mut first_err = None;
    for entry in fs::read_dir(path)? {
        if let Err(e) = entry.and_then(|e| remove(&e.path())) {
            first_err.get_or_insert(e);
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    if scan::trash_dir().as_deref() == Some(path) {
        if !cfg!(target_os = "macos") {
            // freedesktop trash layout
            fs::create_dir_all(path.join("files"))?;
            fs::create_dir_all(path.join("info"))?;
        }
        return Ok(());
    }
    fs::remove_dir(path)
}

fn make_writable(path: &Path, meta: &fs::Metadata) -> io::Result<()> {
    let mut perms = meta.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(perms.mode() | 0o700);
    }
    #[cfg(not(unix))]
    perms.set_readonly(false);
    fs::set_permissions(path, perms)
}

/// Carries the user's ticks from `old` (e.g. a partial scan list) over to the same items in `new`.
pub fn keep_selection(old: &[Item], new: &mut [Item]) {
    let before: std::collections::HashMap<_, _> = old
        .iter()
        .map(|i| (&i.path, (i.category, i.selected)))
        .collect();
    for item in new {
        if let Some(&(cat, selected)) = before.get(&item.path)
            && cat == item.category
        {
            item.selected = selected;
        }
    }
}

/// Ticks or unticks every item of `cat` (all categories when None). Selecting never ticks the first copy
/// of a duplicate group, so at least one copy of every file survives a cleaning.
pub fn set_selection(items: &mut [Item], cat: Option<scan::Category>, on: bool) {
    let mut kept = std::collections::HashSet::new();
    for item in items.iter_mut().filter(|i| cat.is_none_or(|c| i.category == c)) {
        let first_copy = item.category == scan::Category::Duplicate && kept.insert(item.group);
        item.selected = on && !first_copy;
    }
}

/// The app's settings folder. Settings saved under the app's former name (cleanSpace) are moved here once,
/// so existing installs keep their install id, consent and app locks.
/// Settings folder of the sandboxed (Mac App Store) build, saved before HOME is changed; see [`use_real_home`].
static SANDBOX_CONFIG: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();

/// Whether this runs in the macOS app sandbox (the Mac App Store build).
pub fn sandboxed() -> bool {
    cfg!(target_os = "macos") && std::env::var_os("APP_SANDBOX_CONTAINER_ID").is_some()
}

/// In the sandbox HOME is the app's container; scanning and the folder shortcuts need the user's real home,
/// so HOME is pointed back at it. Settings stay in the container. Call first thing in `main`, before threads.
pub fn use_real_home() {
    #[cfg(target_os = "macos")]
    if sandboxed() {
        let _ = SANDBOX_CONFIG.set(dirs::config_dir());
        let pw = unsafe { libc::getpwuid(libc::getuid()) };
        if !pw.is_null() {
            let home = unsafe { std::ffi::CStr::from_ptr((*pw).pw_dir) }.to_string_lossy().into_owned();
            // SAFETY: called at startup before any other thread exists.
            unsafe { std::env::set_var("HOME", home) };
        }
    }
}

/// Moves a file or folder to the Trash. The sandboxed build may not ask Finder to do it, so it uses
/// NSFileManager there (Finder's "Put Back" then does not know the original place).
pub fn to_trash(path: &Path) -> Result<(), trash::Error> {
    #[cfg(target_os = "macos")]
    if sandboxed() {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut ctx = trash::TrashContext::default();
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        return ctx.delete(path);
    }
    trash::delete(path)
}

pub fn config_dir() -> Option<std::path::PathBuf> {
    let base = SANDBOX_CONFIG.get().cloned().flatten().or_else(dirs::config_dir)?;
    let dir = base.join("deskmonk");
    let old = base.join("cleanspace");
    if !dir.exists() && old.is_dir() {
        let _ = fs::rename(&old, &dir);
    }
    Some(dir)
}

/// Default scan roots: the given folder plus the Downloads folder (for "old downloads").
/// See [`scan::scan`] for `on_partial`.
pub fn scan_default(
    root: &Path,
    progress: &AtomicUsize,
    mut on_partial: impl FnMut(Vec<Item>, scan::Phase),
) -> Vec<Item> {
    let places = scan::Places::system();
    let mut roots = vec![root];
    roots.extend(places.downloads.as_deref());
    scan::scan(&roots, &places, progress, &mut on_partial)
}

#[cfg(test)]
mod tests {
    #[test]
    fn select_all_keeps_one_copy_per_group() {
        use super::scan::{Category, Item};
        let item = |category, group| Item { path: format!("{category:?}{group}").into(), size: 1, category, group, selected: false };
        let mut items = vec![
            item(Category::Duplicate, 1), item(Category::Duplicate, 1), item(Category::Duplicate, 1),
            item(Category::Duplicate, 2), item(Category::Duplicate, 2), item(Category::Junk, 0),
        ];
        super::set_selection(&mut items, None, true);
        assert_eq!(items.iter().map(|i| i.selected).collect::<Vec<_>>(), [false, true, true, false, true, true]);
        super::set_selection(&mut items, Some(Category::Junk), false);
        assert!(!items[5].selected && items[4].selected);
    }

    /// The one path that destroys data, and it had no test: junk must go for good, the user's own
    /// files must be recoverable from the trash, and a failure must be reported, not counted as freed.
    #[test]
    fn deletes_junk_for_good_and_user_files_to_the_trash() {
        use super::scan::{Category, Item};
        let dir = std::env::temp_dir().join(format!("deskmonk-del-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let junk = dir.join("cache.tmp");
        let mine = dir.join(format!("deskmonk-keepsake-{}.txt", std::process::id()));
        std::fs::write(&junk, vec![b'j'; 10]).unwrap();
        std::fs::write(&mine, vec![b'm'; 20]).unwrap();
        let gone = dir.join("already-deleted.tmp");

        let item = |path: &std::path::Path, size, category| Item {
            path: path.to_path_buf(), size, category, group: 0, selected: true,
        };
        let mut items = vec![
            item(&junk, 10, Category::Junk),
            item(&mine, 20, Category::Duplicate),
            item(&gone, 999, Category::Junk),
        ];
        let deletion = super::Deletion::start(&mut items, false, || {});
        assert!(items.is_empty(), "selected items are taken out of the list");

        let finished = (0..100)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(50));
                deletion.poll()
            })
            .expect("the deletion should finish within five seconds");

        assert!(!junk.exists() && !mine.exists(), "both selected files should be gone from disk");
        // The missing file must be reported, and must not inflate the "space freed" number.
        assert_eq!(finished.freed, 30, "only the two real files count");
        assert_eq!(finished.failed.len(), 1);
        assert_eq!(finished.failed[0].path, gone);
        assert!(finished.first_error.is_some());

        // Junk is unrecoverable; the user's file is sitting in the trash. Only ours is touched.
        let listed = trash::os_limited::list().unwrap_or_default();
        let mut mine_in_trash: Vec<_> = listed
            .into_iter()
            .filter(|i| i.name.to_string_lossy().starts_with(&format!("deskmonk-keepsake-{}", std::process::id())))
            .collect();
        assert_eq!(mine_in_trash.len(), 1, "the user's own file must be recoverable from the trash");
        trash::os_limited::purge_all(mine_in_trash.drain(..)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn removes_read_only_folders() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("deskmonk-rm-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("pkg/sub")).unwrap();
        std::fs::write(dir.join("pkg/sub/file"), "x").unwrap();
        for d in ["pkg", "pkg/sub"] {
            std::fs::set_permissions(dir.join(d), std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        super::remove(&dir).unwrap();
        assert!(!dir.exists());
    }
}
