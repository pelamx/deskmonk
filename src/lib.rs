pub mod applock;
pub mod diskmap;
pub mod focus;
pub mod hardware;
pub mod plan;
pub mod i18n;
pub mod scan;
pub mod speedtest;
pub mod telemetry;
pub mod update;
pub mod vault;

use scan::Item;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
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
    /// Items that could not be deleted (completely); unticked, so Clean does not keep offering what just failed.
    pub failed: Vec<Item>,
    pub freed: u64,
    /// Reason of the first failure, for the status line.
    pub first_error: Option<String>,
    /// Items that went to the Trash, with their place there, for [`Undo`]. macOS only: Finder's Put Back
    /// does not know items moved there without Finder, and the other systems' trash can restore them itself.
    pub trashed: Vec<(Item, PathBuf)>,
}

impl Finished {
    /// Puts what could not be deleted back into `items` and returns what the cleaning can undo.
    pub fn into_list(self, items: &mut Vec<Item>) -> Undo {
        let dropped = return_to_list(items, self.failed);
        Undo { trashed: self.trashed, dropped }
    }
}

/// What the last cleaning sent to the Trash, to put it back where it was.
#[derive(Default)]
pub struct Undo {
    trashed: Vec<(Item, PathBuf)>,
    /// Copies that left the list because their others went; they come back with them.
    dropped: Vec<Item>,
}

impl Undo {
    pub fn len(&self) -> usize {
        self.trashed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.trashed.is_empty()
    }

    /// Moves the items back from the Trash and into `items` (unticked). Returns how many went back and the
    /// first reason one could not.
    pub fn put_back(self, items: &mut Vec<Item>) -> (usize, Option<String>) {
        let (mut back, mut first_error) = (self.dropped, None);
        let mut n = 0;
        for (mut item, place) in self.trashed {
            let result = if item.path.symlink_metadata().is_ok() {
                Err(io::Error::new(io::ErrorKind::AlreadyExists, "a file with this name is there again"))
            } else {
                item.path.parent().map_or(Ok(()), fs::create_dir_all).and_then(|()| fs::rename(&place, &item.path))
            };
            match result {
                Ok(()) => {
                    item.selected = false;
                    back.push(item);
                    n += 1;
                }
                Err(e) => {
                    first_error.get_or_insert_with(|| format!("{e} ({})", item.path.display()));
                }
            }
        }
        return_to_list(items, back);
        (n, first_error)
    }
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
                trashed: Vec::new(),
            };
            for mut item in selected {
                let outcome = if is_permanent(&item, permanent) {
                    remove(&item.path).map(|()| None).map_err(|e| e.to_string())
                } else {
                    to_trash(&item.path)
                };
                match outcome {
                    Ok(place) => {
                        result.freed += item.size;
                        if let Some(place) = place {
                            result.trashed.push((item, place));
                        }
                    }
                    Err(e) => {
                        // Reason first: the status line is cut at its end, and long paths hid it.
                        result
                            .first_error
                            .get_or_insert_with(|| format!("{}: {e}", item.name()));
                        item.selected = false;
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

/// Puts items (that could not be deleted, or were put back) into the list where a scan lists them, so a
/// duplicate sits with its group again, then takes off duplicates left with no other copy and returns those.
fn return_to_list(items: &mut Vec<Item>, back: Vec<Item>) -> Vec<Item> {
    items.extend(back);
    // The scan's order: junk, then duplicates by group with the kept copy first, then the rest biggest first.
    let rank = |i: &Item| match i.category {
        scan::Category::Junk => 0,
        scan::Category::Duplicate => 1,
        _ => 2,
    };
    items.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then_with(|| match a.category {
            scan::Category::Duplicate => (a.group, &a.path).cmp(&(b.group, &b.path)),
            _ => b.size.cmp(&a.size),
        })
    });
    drop_lone_copies(items)
}

/// Takes duplicates whose copies are all gone off the list: the one left isn't a duplicate any more.
fn drop_lone_copies(items: &mut Vec<Item>) -> Vec<Item> {
    let mut members = std::collections::HashMap::new();
    for item in items.iter().filter(|i| i.category == scan::Category::Duplicate) {
        *members.entry(item.group).or_insert(0) += 1;
    }
    let lone;
    (*items, lone) = std::mem::take(items)
        .into_iter()
        .partition(|i| i.category != scan::Category::Duplicate || members[&i.group] > 1);
    lone
}

/// Count and size of what a cleaning could remove (of `cat`, or all when None): the first copy of a duplicate
/// group stays, see [`set_selection`], so it isn't counted.
pub fn removable(items: &[Item], cat: Option<scan::Category>) -> (usize, u64) {
    let mut kept = std::collections::HashSet::new();
    items
        .iter()
        .filter(|i| cat.is_none_or(|c| i.category == c))
        .filter(|i| i.category != scan::Category::Duplicate || !kept.insert(i.group))
        .fold((0, 0), |(n, s), i| (n + 1, s + i.size))
}

/// Ticks or unticks every item of `cat` (all categories when None). Selecting never ticks the first copy
/// of a duplicate group, so at least one copy of every file survives a cleaning. Returns how many are ticked.
pub fn set_selection(items: &mut [Item], cat: Option<scan::Category>, on: bool) -> usize {
    let mut kept = std::collections::HashSet::new();
    let mut ticked = 0;
    for item in items.iter_mut().filter(|i| cat.is_none_or(|c| i.category == c)) {
        let first_copy = item.category == scan::Category::Duplicate && kept.insert(item.group);
        item.selected = on && !first_copy;
        ticked += item.selected as usize;
    }
    ticked
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

/// Moves a file or folder to the Trash and returns its place there (macOS only, see [`put_back`]).
/// On macOS through NSFileManager: asking Finder plays its trash sound for every item and takes about 150 ms
/// each, and the sandboxed build may not ask it at all. Finder's "Put Back" then does not know the original place.
pub fn to_trash(path: &Path) -> Result<Option<PathBuf>, String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSFileManager, NSURL};
        let url = NSURL::from_file_path(path).ok_or_else(|| format!("not a file path: {}", path.display()))?;
        // Runs on the deletion thread, which has no pool of its own for Foundation's temporary objects.
        objc2::rc::autoreleasepool(|_| {
            let mut place = None;
            NSFileManager::defaultManager()
                .trashItemAtURL_resultingItemURL_error(&url, Some(&mut place))
                .map_err(|e| e.localizedDescription().to_string())?;
            Ok(place.and_then(|u| u.to_file_path()))
        })
    }
    #[cfg(not(target_os = "macos"))]
    trash::delete(path).map(|()| None).map_err(|e| e.to_string())
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
    fn failed_copies_rejoin_their_group_and_put_back_restores_the_list() {
        use super::scan::{Category::*, Item};
        let dir = std::env::temp_dir().join(format!("deskmonk-undo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let item = |name: &str, category, group, size| Item { path: dir.join(name), size, category, group, selected: false };
        let names = |items: &[Item]| items.iter().map(|i| i.path.file_name().unwrap().to_str().unwrap().to_owned()).collect::<Vec<_>>();
        // Left on the list while a2, a3 and b2 were being cleaned: a3 failed, a2 and b2 went to the "Trash" (bin).
        let mut items = vec![item("junk", Junk, 0, 5), item("a1", Duplicate, 1, 10), item("b1", Duplicate, 2, 10), item("big", Large, 0, 99)];
        for name in ["a2", "b2"] {
            std::fs::write(dir.join("bin").join(name), name).unwrap();
        }
        let failed = Item { selected: true, ..item("a3", Duplicate, 1, 10) };
        let trashed = ["a2", "b2"].map(|n| (Item { selected: true, ..item(n, Duplicate, n.starts_with('b') as usize + 1, 10) }, dir.join("bin").join(n)));
        let done = super::Finished { failed: vec![failed], freed: 20, first_error: None, trashed: trashed.into() };
        let undo = done.into_list(&mut items);
        assert_eq!(names(&items), ["junk", "a1", "a3", "big"], "a3 is back with its group, under one heading; b1 is alone");
        assert!(items[2].selected, "the failed one stays ticked");
        assert_eq!(super::set_selection(&mut items, Some(Duplicate), true), 1, "a1 stays, a3 can be ticked again");
        assert_eq!(super::set_selection(&mut [], None, true), 0, "nothing to tick in an empty list");

        // Something new took b2's place meanwhile: that one stays in the bin, a2 goes home.
        std::fs::write(dir.join("b2"), "new").unwrap();
        let (n, err) = undo.put_back(&mut items);
        assert_eq!(n, 1);
        assert!(err.is_some_and(|e| e.contains("there again")));
        assert_eq!(std::fs::read(dir.join("a2")).unwrap(), b"a2");
        assert!(dir.join("bin/b2").exists() && !dir.join("bin/a2").exists());
        assert_eq!(names(&items), ["junk", "a1", "a2", "a3", "big"], "a2 back in its place; b1 still has no other copy listed");
        assert!(!items[2].selected, "what comes back is not ticked");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn counts_leave_out_the_copy_that_stays() {
        use super::scan::{Category, Item};
        let item = |name: &str, category, group, selected| Item { path: name.into(), size: 10, category, group, selected };
        let mut items = vec![
            item("a1", Category::Duplicate, 1, false), item("a2", Category::Duplicate, 1, false), item("a3", Category::Duplicate, 1, false),
            item("b1", Category::Duplicate, 2, false), item("b2", Category::Duplicate, 2, true),
            item("big", Category::Large, 0, false),
        ];
        assert_eq!(super::removable(&items, None), (4, 40)); // a2, a3, b2, big
        assert_eq!(super::removable(&items, Some(Category::Duplicate)), (3, 30));
        assert_eq!(super::removable(&items, Some(Category::Large)), (1, 10));
        // b2 is ticked: taking it away leaves b1 alone, while group a still has its copies.
        items.retain(|i| !i.selected);
        super::drop_lone_copies(&mut items);
        let left: Vec<_> = items.iter().map(|i| i.path.to_str().unwrap()).collect();
        assert_eq!(left, ["a1", "a2", "a3", "big"]);
    }

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

    #[test]
    fn failed_items_come_back_unticked_with_the_reason() {
        use super::scan::{Category, Item};
        let path = std::env::temp_dir().join(format!("deskmonk-gone-{}", std::process::id()));
        let mut items = vec![Item { path, size: 1, category: Category::Junk, group: 0, selected: true }];
        let done = super::Deletion::start(&mut items, false, || {}).rx.recv().unwrap();
        assert!(items.is_empty() && done.freed == 0);
        assert!(!done.failed[0].selected);
        assert!(done.first_error.unwrap().starts_with(&done.failed[0].name()));
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
