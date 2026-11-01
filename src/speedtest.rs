//! Sequential disk speed test: writes a temporary file, reads it back, always deletes it.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Bytes per second.
pub struct Speed {
    pub write: f64,
    pub read: f64,
}

const BLOCK: usize = 8 << 20;

/// Removes the test file however the test ends.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Where to test: on a real disk. On Linux the temp folder is often tmpfs, which is RAM: it would report RAM's speed,
/// and has too little room for the test anyway.
pub fn test_dir() -> PathBuf {
    if cfg!(target_os = "linux") {
        for dir in [dirs::cache_dir(), dirs::home_dir()].into_iter().flatten() {
            if fs::create_dir_all(&dir).is_ok() {
                return dir;
            }
        }
    }
    std::env::temp_dir()
}

/// A test cut off by a crash or a kill leaves its file behind, and outside the temp folder nothing else removes it.
fn remove_leftovers(dir: &Path) {
    for path in fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) {
        let pid = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix(".deskmonk-speedtest-")).map(str::to_owned);
        if pid.is_some_and(|pid| !Path::new("/proc").join(pid).exists()) {
            let _ = fs::remove_file(path);
        }
    }
}

/// Writes then reads `size` bytes in `dir`. `progress` counts bytes done, up to 2 × `size`.
/// Refuses to run when the disk does not have plenty of free space: users of a cleaner are often nearly full.
/// Slow (seconds on an HDD): call it off the UI thread.
pub fn run(dir: &Path, size: u64, progress: &AtomicU64) -> io::Result<Speed> {
    if cfg!(target_os = "linux") {
        remove_leftovers(dir);
    }
    // shortcut: no free-space check on Windows (see disk_space); a full disk fails the write and the file is still removed.
    if let Some((_, free)) = crate::disk_space(dir)
        && free < size.saturating_mul(3)
    {
        return Err(io::Error::other(crate::i18n::t(
            "Test için yeterli boş alan yok",
            "Not enough free space for the test",
            "No hay suficiente espacio libre para la prueba",
        )));
    }
    let file = TempFile(dir.join(format!(".deskmonk-speedtest-{}", std::process::id())));
    // Random-looking data: some SSD controllers compress zeros and would report fake speeds.
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let block: Vec<u8> = (0..BLOCK)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();

    let mut out = File::create(&file.0)?;
    no_cache(&out);
    let start = Instant::now();
    let mut done = 0;
    while done < size {
        let n = BLOCK.min((size - done) as usize);
        out.write_all(&block[..n])?;
        done += n as u64;
        progress.store(done, Ordering::Relaxed);
    }
    out.sync_all()?;
    let write = size as f64 / start.elapsed().as_secs_f64();
    drop_cache(&out);
    drop(out);

    let mut input = File::open(&file.0)?;
    no_cache(&input);
    let mut buf = vec![0; BLOCK];
    let start = Instant::now();
    let mut done = 0;
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        done += n as u64;
        progress.store(size + done, Ordering::Relaxed);
    }
    let read = done as f64 / start.elapsed().as_secs_f64();
    Ok(Speed { write, read })
}

/// macOS: bypass the page cache, so reads come from the disk and not from RAM.
#[cfg(target_os = "macos")]
fn no_cache(file: &File) {
    use std::os::fd::AsRawFd;
    unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) };
}

#[cfg(not(target_os = "macos"))]
fn no_cache(_: &File) {}

/// Linux: evict the written file from the page cache before reading it back.
#[cfg(target_os = "linux")]
fn drop_cache(file: &File) {
    use std::os::fd::AsRawFd;
    unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
}

// shortcut: on Windows the read pass may be served from the cache (looks too fast); open with
// FILE_FLAG_NO_BUFFERING and sector-aligned buffers if Windows numbers matter.
#[cfg(not(target_os = "linux"))]
fn drop_cache(_: &File) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_and_cleans_up() {
        let dir = std::env::temp_dir();
        let progress = AtomicU64::new(0);
        let speed = run(&dir, 16 << 20, &progress).unwrap();
        assert!(speed.write > 0.0 && speed.read > 0.0);
        assert_eq!(progress.load(Ordering::Relaxed), 32 << 20);
        assert!(!dir.join(format!(".deskmonk-speedtest-{}", std::process::id())).exists());
        assert!(run(&dir, u64::MAX / 4, &progress).is_err(), "must refuse without free space");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn tests_on_a_disk_and_removes_only_dead_leftovers() {
        assert_ne!(test_dir(), std::env::temp_dir());
        let dir = std::env::temp_dir().join(format!("deskmonk-speed-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // A pid no process can have, this process's own, and someone else's file.
        let dead = dir.join(".deskmonk-speedtest-999999999");
        let alive = dir.join(format!(".deskmonk-speedtest-{}", std::process::id()));
        let other = dir.join("notes.txt");
        for file in [&dead, &alive, &other] {
            fs::write(file, "x").unwrap();
        }
        remove_leftovers(&dir);
        assert!(!dead.exists() && alive.exists() && other.exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
