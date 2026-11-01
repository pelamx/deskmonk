use crate::i18n::t;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};
use walkdir::WalkDir;

pub const LARGE_FILE: u64 = 500 * 1024 * 1024;
pub const OLD_DOWNLOAD: Duration = Duration::from_secs(90 * 24 * 3600);
/// Smaller duplicates free almost nothing and are mostly app internals.
const DUP_MIN: u64 = 32 * 1024;
/// Never descended into: deleting anything inside breaks repos / projects, and they are huge to walk.
const SKIP_DIRS: &[&str] = &[".git", "node_modules"];

const JUNK_NAMES: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini"];
const JUNK_EXT: &[&str] = &[
    "tmp",
    "temp",
    "bak",
    "old",
    "log",
    "crdownload",
    "part",
    "dmp",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Duplicate,
    Junk,
    OldDownload,
    Large,
}

impl Category {
    pub const ALL: [Category; 4] = [Self::Duplicate, Self::Junk, Self::OldDownload, Self::Large];

    pub fn label(self) -> &'static str {
        match self {
            Self::Duplicate => t("Kopyalar", "Duplicates", "Duplicados"),
            Self::Junk => t("Gereksiz dosyalar", "Junk files", "Archivos basura"),
            Self::OldDownload => t("Eski indirmeler", "Old downloads", "Descargas antiguas"),
            Self::Large => t("Büyük dosyalar", "Large files", "Archivos grandes"),
        }
    }
}

#[derive(Clone)]
pub struct Item {
    pub path: PathBuf,
    pub size: u64,
    pub category: Category,
    /// Duplicate group number; 0 for other categories.
    pub group: usize,
    pub selected: bool,
}

impl Item {
    /// Name to show: the file or folder name, "Trash" for the OS trash.
    pub fn name(&self) -> String {
        if trash_dir().as_ref() == Some(&self.path) {
            return t("Çöp Kutusu", "Trash", "Papelera").into();
        }
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

fn is_junk(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    JUNK_NAMES.contains(&name) || name.ends_with('~') || JUNK_EXT.contains(&ext.as_str())
}

/// Folders whose whole content is safe to delete: caches regenerate, the trash is already "deleted".
/// Each first-level entry inside one becomes a single Junk item (e.g. ~/.cache/google-chrome).
pub fn junk_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let home = dirs::home_dir();
    if cfg!(windows) {
        // dirs::cache_dir() is all of %LOCALAPPDATA% on Windows (real app data), so only %TEMP%.
        roots.push(std::env::temp_dir());
    } else {
        roots.extend(dirs::cache_dir()); // ~/.cache, ~/Library/Caches
    }
    roots.extend(trash_dir());
    // Flatpak apps keep their cache in their own sandbox folder, not in ~/.cache.
    if let Some(h) = &home {
        roots.extend(fs::read_dir(h.join(".var/app")).into_iter().flatten().flatten().map(|e| e.path().join("cache")).filter(|c| c.is_dir()));
    }
    roots.extend(home.map(|h| h.join(".npm").join("_cacache")));
    roots
}

/// Well-known folders the classification depends on.
pub struct Places {
    pub downloads: Option<PathBuf>,
    /// See [`junk_roots`].
    pub junk: Vec<PathBuf>,
    /// The user's own document/media folders.
    pub user: Vec<PathBuf>,
    /// Listed as one item instead of per entry. See [`trash_dir`].
    pub trash: Option<PathBuf>,
}

impl Places {
    pub fn system() -> Self {
        let user = [
            dirs::download_dir(),
            dirs::document_dir(),
            dirs::desktop_dir(),
            dirs::picture_dir(),
            dirs::audio_dir(),
            dirs::video_dir(),
        ];
        Self {
            downloads: dirs::download_dir(),
            junk: junk_roots(),
            user: user.into_iter().flatten().collect(),
            trash: trash_dir(),
        }
    }
}

/// The OS trash folder. None on Windows, whose Recycle Bin is not a plain folder.
pub fn trash_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        dirs::home_dir().map(|h| h.join(".Trash"))
    } else if cfg!(windows) {
        None
    } else {
        dirs::data_dir().map(|d| d.join("Trash"))
    }
}

/// Items in the OS trash must be removed for good, moving them "to the trash" again is meaningless.
pub fn is_in_trash(path: &Path) -> bool {
    trash_dir().is_some_and(|t| path.starts_with(t))
}

/// Which step a partial scan result comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Still listing files; more results will come.
    Walking,
    /// Listing done; only duplicates are still missing.
    Comparing,
}

/// Hash of the whole file, or only its first `limit` bytes.
fn hash_file(path: &Path, limit: Option<u64>) -> io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    io::copy(
        &mut io::Read::take(File::open(path)?, limit.unwrap_or(u64::MAX)),
        &mut hasher,
    )?;
    Ok(hasher.finalize().into())
}

/// Splits `paths` into groups with equal hash, dropping singletons.
fn same_hash(paths: Vec<PathBuf>, limit: Option<u64>) -> Vec<Vec<PathBuf>> {
    let mut by_hash: HashMap<[u8; 32], Vec<PathBuf>> = HashMap::new();
    for path in paths {
        if let Ok(h) = hash_file(&path, limit) {
            by_hash.entry(h).or_default().push(path);
        }
    }
    by_hash.into_values().filter(|g| g.len() > 1).collect()
}

/// Scans `roots` (symlinks not followed). Each file lands in at most one category so sizes are never double-counted.
/// Content of `places.junk` folders found under a root is reported per first-level entry instead of per file.
/// `on_partial` gets everything found so far except duplicates about once a second while walking and once
/// when the walk is done, because comparing contents for duplicates is the slowest step.
/// The returned list is the final one and replaces the partial ones.
pub fn scan(
    roots: &[&Path],
    places: &Places,
    progress: &AtomicUsize,
    mut on_partial: impl FnMut(Vec<Item>, Phase),
) -> Vec<Item> {
    let now = SystemTime::now();
    // Duplicate candidates by size: only user files (not hidden, not in a git repo) big enough to matter.
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    let mut flagged: Vec<Item> = Vec::new(); // junk by name, old downloads, large files
    let mut junk_dirs: HashMap<PathBuf, u64> = HashMap::new();
    let mut repos: HashSet<PathBuf> = HashSet::new();
    let mut read_only: HashSet<PathBuf> = HashSet::new(); // folders this user may not write to right now
    let mut locked: HashSet<PathBuf> = HashSet::new(); // the same and not ours: their content cannot be deleted at all
    let mut stuck: HashSet<PathBuf> = HashSet::new(); // junk items with such a folder inside
    let mut last_report = Instant::now();
    // The whole trash is one item; caches are one item per app folder.
    let junk_key = |path: &Path| {
        let (jroot, rel) = places.junk.iter().find_map(|j| Some((j, path.strip_prefix(j).ok()?)))?;
        if places.trash.as_ref() == Some(jroot) {
            Some(jroot.clone())
        } else {
            Some(jroot.join(rel.components().next()?))
        }
    };

    // A root inside another root would be walked twice and its files would look like duplicates of themselves.
    let roots: Vec<&Path> = roots
        .iter()
        .enumerate()
        .filter(|&(i, r)| {
            !roots
                .iter()
                .enumerate()
                .any(|(j, o)| j != i && r.starts_with(o) && (r != o || j < i))
        })
        .map(|(_, r)| *r)
        .collect();
    for root in roots {
        let walk = WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| !SKIP_DIRS.contains(&e.file_name().to_str().unwrap_or("")));
        for entry in walk.filter_map(Result::ok) {
            if last_report.elapsed() > Duration::from_secs(1) {
                on_partial(snapshot(&junk_dirs, &flagged), Phase::Walking);
                last_report = Instant::now();
            }
            // Directories come before their content, so a repo is known before its files.
            if entry.file_type().is_dir() && entry.path().join(".git").exists() {
                repos.insert(entry.path().to_path_buf());
            }
            if entry.file_type().is_dir() && is_read_only(&entry) {
                read_only.insert(entry.path().to_path_buf());
                if !is_mine(&entry) {
                    locked.insert(entry.path().to_path_buf());
                }
            }
            // Clean could never delete this, so it is not offered.
            // shortcut: a cache with one locked subfolder is left out whole; list the deletable rest if that gets common.
            if entry.path().parent().is_some_and(|p| locked.contains(p)) {
                if let Some(key) = junk_key(entry.path()) {
                    junk_dirs.remove(&key);
                    stuck.insert(key);
                }
                continue;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            progress.fetch_add(1, Ordering::Relaxed);
            let path = entry.into_path();
            let size = meta.len();
            if let Some(key) = junk_key(&path) {
                if !stuck.contains(&key) {
                    *junk_dirs.entry(key).or_default() += size;
                }
                continue;
            }
            // Files cannot leave a read-only folder, and it is usually read-only on purpose: Go's module cache
            // (~/go/pkg/mod) and the like break when files go missing. Whole caches are junk roots, handled above.
            if path.parent().is_some_and(|p| read_only.contains(p)) {
                continue;
            }
            let modified = meta.modified().unwrap_or(now);
            let old = now.duration_since(modified).unwrap_or_default() > OLD_DOWNLOAD;
            let class = if is_junk(&path) {
                Some((Category::Junk, true))
            } else if old
                && places
                    .downloads
                    .as_ref()
                    .is_some_and(|d| path.starts_with(d))
            {
                Some((Category::OldDownload, false))
            } else if size >= LARGE_FILE {
                Some((Category::Large, false))
            } else {
                None
            };
            if let Some((category, selected)) = class {
                flagged.push(Item {
                    path: path.clone(),
                    size,
                    category,
                    group: 0,
                    selected,
                });
            }
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let hidden = rel.parent().is_some_and(|p| {
                p.components()
                    .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
            });
            if size >= DUP_MIN && !hidden && !path.ancestors().any(|a| repos.contains(a)) {
                by_size.entry(size).or_default().push(path);
            }
        }
    }
    on_partial(snapshot(&junk_dirs, &flagged), Phase::Comparing);

    // Duplicates: same size, then same first 64 KB, then same full content.
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    for paths in by_size.into_values().filter(|p| p.len() > 1) {
        for candidates in same_hash(paths, Some(64 * 1024)) {
            groups.extend(same_hash(candidates, None));
        }
    }
    // Sort members first so each group's kept copy and the group order are deterministic.
    groups.iter_mut().for_each(|g| g.sort());
    groups.sort();

    let mut items = snapshot(&junk_dirs, &[]);
    let sizes: HashMap<&PathBuf, u64> = flagged.iter().map(|i| (&i.path, i.size)).collect();
    let mut dup_items = Vec::new();
    for (i, group) in groups.into_iter().enumerate() {
        let size = sizes
            .get(&group[0])
            .copied()
            .unwrap_or_else(|| fs::metadata(&group[0]).map_or(0, |m| m.len()));
        for (j, path) in group.into_iter().enumerate() {
            // The first copy is always kept. Extra copies are pre-selected only in the user's own folders;
            // elsewhere they may be files an installed app needs.
            let selected = j > 0 && places.user.iter().any(|u| path.starts_with(u));
            dup_items.push(Item {
                path,
                size,
                category: Category::Duplicate,
                group: i + 1,
                selected,
            });
        }
    }
    // A duplicate is listed only as a duplicate.
    let dup_paths: HashSet<&PathBuf> = dup_items.iter().map(|i| &i.path).collect();
    flagged.retain(|i| !dup_paths.contains(&i.path));
    items.extend(dup_items);
    flagged.sort_by_key(|i| std::cmp::Reverse(i.size));
    items.extend(flagged);
    items
}

/// Whether this user may not create or remove entries in the folder right now.
#[cfg(unix)]
fn is_read_only(dir: &walkdir::DirEntry) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(dir.path().as_os_str().as_bytes()) else { return false };
    unsafe { libc::access(path.as_ptr(), libc::W_OK | libc::X_OK) != 0 }
}

/// Whether this user owns it, and so can make it writable. A root-owned folder in ~/.cache is not.
#[cfg(unix)]
fn is_mine(dir: &walkdir::DirEntry) -> bool {
    use std::os::unix::fs::MetadataExt;
    dir.metadata().is_ok_and(|m| m.uid() == unsafe { libc::geteuid() })
}

#[cfg(not(unix))]
fn is_read_only(_: &walkdir::DirEntry) -> bool {
    false
}

#[cfg(not(unix))]
fn is_mine(_: &walkdir::DirEntry) -> bool {
    true
}

/// Junk folders plus flagged files, biggest first within each.
fn snapshot(junk_dirs: &HashMap<PathBuf, u64>, flagged: &[Item]) -> Vec<Item> {
    let mut junk: Vec<Item> = junk_dirs
        .iter()
        .map(|(path, &size)| Item {
            path: path.clone(),
            size,
            category: Category::Junk,
            group: 0,
            selected: true,
        })
        .collect();
    junk.sort_by_key(|i| std::cmp::Reverse(i.size));
    let mut rest = flagged.to_vec();
    rest.sort_by_key(|i| std::cmp::Reverse(i.size));
    junk.extend(rest);
    junk
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn classifies_files() {
        let dir = std::env::temp_dir().join(format!("deskmonk-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for d in [
            "sub",
            ".config",
            "node_modules/pkg",
            "cache/app/deep",
            "repo/.git",
            "repo/src",
            "trash/files",
            "trash/info",
        ] {
            fs::create_dir_all(dir.join(d)).unwrap();
        }
        let big = vec![7u8; 40_000];
        let mut other = big.clone();
        other[39_999] = 8; // same size and same first 64 KB check passes only on full hash
        fs::write(dir.join("a.bin"), &big).unwrap();
        fs::write(dir.join("sub/b.bin"), &big).unwrap();
        fs::write(dir.join("c.bin"), &other).unwrap();
        fs::write(dir.join(".config/d.bin"), &big).unwrap(); // hidden: not a duplicate candidate
        fs::write(dir.join("node_modules/pkg/e.bin"), &big).unwrap(); // skipped entirely
        fs::write(dir.join("repo/src/f.bin"), &big).unwrap(); // inside a git repo: not a candidate
        fs::write(dir.join("x.tmp"), "junk").unwrap();
        fs::write(dir.join(".DS_Store"), "j").unwrap();
        fs::write(dir.join("cache/app/1"), "aaaa").unwrap();
        fs::write(dir.join("cache/app/deep/2"), "bb").unwrap();
        fs::write(dir.join("trash/files/old.txt"), vec![1u8; 100]).unwrap();
        fs::write(dir.join("trash/info/old.txt.trashinfo"), vec![1u8; 10]).unwrap();

        let progress = AtomicUsize::new(0);
        let places = Places {
            trash: Some(dir.join("trash")),
            downloads: None,
            junk: vec![dir.join("cache"), dir.join("trash")],
            user: vec![dir.join("sub")],
        };
        let mut partial = Vec::new();
        // Passing a root twice and a root inside another must not create self-duplicates.
        let items = scan(
            &[&dir, &dir.join("sub"), &dir],
            &places,
            &progress,
            |p, _| partial = p,
        );
        assert_eq!(
            partial
                .iter()
                .filter(|i| i.category == Category::Junk)
                .count(),
            4
        );
        assert!(partial.iter().all(|i| i.category != Category::Duplicate));
        fs::remove_dir_all(&dir).unwrap();

        let of = |c: Category| items.iter().filter(|i| i.category == c).collect::<Vec<_>>();
        let dups = of(Category::Duplicate);
        assert_eq!(dups.len(), 2);
        assert!(dups[0].path.ends_with("a.bin") && !dups[0].selected);
        assert!(dups[1].path.ends_with("sub/b.bin") && dups[1].selected);
        let junk = of(Category::Junk);
        assert_eq!(junk.len(), 4);
        assert!(junk[0].path.ends_with("trash") && junk[0].size == 110); // the whole trash is one item
        assert!(junk[1].path.ends_with("cache/app") && junk[1].size == 6); // whole cache folder as one item
        assert_eq!(progress.load(Ordering::Relaxed), 11); // node_modules and .git not walked
        assert_eq!(items.len(), 6);
        assert_eq!(human_size(1536), "1.5 KB");
    }

    #[cfg(unix)]
    #[test]
    fn does_not_offer_files_in_read_only_folders() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // root may write anywhere
        }
        let dir = std::env::temp_dir().join(format!("deskmonk-ro-{}", std::process::id()));
        fs::create_dir_all(dir.join("mod@v1")).unwrap();
        fs::create_dir_all(dir.join("mine")).unwrap();
        let body = vec![7u8; DUP_MIN as usize];
        fs::write(dir.join("mod@v1/a.bin"), &body).unwrap();
        fs::write(dir.join("mine/a.bin"), &body).unwrap();
        fs::write(dir.join("mine/b.bin"), &body).unwrap();
        fs::set_permissions(dir.join("mod@v1"), fs::Permissions::from_mode(0o555)).unwrap();
        let places = Places { trash: None, downloads: None, junk: vec![], user: vec![] };
        let items = scan(&[&dir], &places, &AtomicUsize::new(0), |_, _| {});
        fs::set_permissions(dir.join("mod@v1"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(items.len(), 2); // the two copies in the writable folder
        assert!(items.iter().all(|i| i.path.parent().unwrap().ends_with("mine")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn does_not_offer_what_cannot_be_deleted() {
        if unsafe { libc::geteuid() } == 0 {
            return; // root may delete anything
        }
        let etc = Path::new("/etc"); // root-owned everywhere
        let places = Places { trash: None, downloads: None, junk: vec![etc.into()], user: vec![] };
        let items = scan(&[etc], &places, &AtomicUsize::new(0), |_, _| {});
        assert!(items.is_empty());
    }
}
