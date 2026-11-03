//! App lock against real processes: a locked program is stopped and reported, the password opens it,
//! and once it quits it is locked again.
#![cfg(target_os = "linux")]

use deskmonk::applock::Watcher;
use std::process::{Child, Command};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

/// Waits up to 5 s for `done`.
fn eventually(mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn exited(child: &mut Child) -> bool {
    child.try_wait().unwrap().is_some()
}

/// Our own copy of the program runs, so the lock can't touch the user's processes.
fn running_copies(exe: &std::path::Path) -> Vec<u32> {
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|e| Some((e.file_name().to_str()?.parse().ok()?, std::fs::read_link(e.path().join("exe")).ok()?)))
        .filter(|(_, p): &(u32, std::path::PathBuf)| p == exe)
        .map(|(pid, _)| pid)
        .collect()
}

/// Brave, Chrome, Signal and Spotify start this way on Arch: the launcher runs a script that execs the real program.
#[test]
fn locks_a_program_started_through_a_wrapper_script() {
    let dir = std::env::temp_dir().join(format!("deskmonk-lock-wrapper-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("opt")).unwrap();
    let exe = dir.join("opt/wrapped-sleep");
    std::fs::copy("/bin/sleep", &exe).unwrap();
    let exe = std::fs::canonicalize(exe).unwrap();
    let script = dir.join("wrapped");
    std::fs::write(&script, format!("#!/bin/sh\nexport A=1\nexec \"{}\" \"$@\"\n", exe.display())).unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let app = dir.join("wrapped.desktop");
    std::fs::write(&app, format!("[Desktop Entry]\nType=Application\nName=Wrapped\nExec=\"{}\" 30 %U\n", script.display())).unwrap();

    let (tx, rx) = channel();
    let _watcher = Watcher::start(vec![app.clone()], false, move |_, a| tx.send(a).unwrap());
    let mut child = Command::new(&script).arg("30").spawn().unwrap();
    let stopped = eventually(|| exited(&mut child));
    let _ = child.kill();
    let _ = child.wait();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(stopped, "program started through a wrapper script kept running");
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), app);
}

/// A program open before the lock is left alone, until it is launched again. A second launch of a browser hands
/// over to the running one and is gone in about 60 ms, so the launch here is that short too.
#[test]
fn a_program_left_open_quits_when_launched_again_even_briefly() {
    let dir = std::env::temp_dir().join(format!("deskmonk-lock-again-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("again-sleep");
    std::fs::copy("/bin/sleep", &exe).unwrap();
    let exe = std::fs::canonicalize(exe).unwrap();
    let app = dir.join("again.desktop");
    std::fs::write(&app, format!("[Desktop Entry]\nType=Application\nName=Again\nExec=\"{}\" 30\n", exe.display())).unwrap();

    let mut open = Command::new(&exe).arg("30").spawn().unwrap();
    assert!(eventually(|| running_copies(&exe).len() == 1));
    let (tx, rx) = channel();
    let watcher = Watcher::start(vec![app.clone()], false, move |_, a| tx.send(a).unwrap());

    // Left alone while nothing is launched.
    std::thread::sleep(Duration::from_millis(1000));
    assert!(!exited(&mut open), "program left open was stopped without a new launch");

    // A very short launch: the open program is asked to quit, and the password is asked for.
    let mut again = Command::new(&exe).arg("0.05").spawn().unwrap();
    let quit = eventually(|| exited(&mut open));
    let asked = rx.recv_timeout(Duration::from_secs(5));
    let _ = open.kill();
    let _ = open.wait();
    let _ = again.wait();
    drop(watcher);
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(quit, "a short new launch did not stop the program left open");
    assert_eq!(asked.unwrap(), app);
}

#[test]
fn locks_unlocks_and_relocks_a_real_program() {
    let dir = std::env::temp_dir().join(format!("deskmonk-lock-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("locked-sleep");
    std::fs::copy("/bin/sleep", &exe).unwrap();
    let exe = std::fs::canonicalize(exe).unwrap();
    let app = dir.join("locked-sleep.desktop");
    std::fs::write(&app, format!("[Desktop Entry]\nType=Application\nName=Sleepy\nExec=\"{}\" 30 %U\n", exe.display())).unwrap();

    let (tx, rx) = channel();
    let watcher = Watcher::start(vec![app.clone()], false, move |_, a| tx.send(a).unwrap());

    // Launched: stopped, and the password is asked for this app.
    let mut first = Command::new(&exe).arg("30").spawn().unwrap();
    assert!(eventually(|| exited(&mut first)), "locked program kept running");
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), app);

    // Right password: deskMonk opens it (field code dropped), and it keeps running.
    watcher.unlock(&app);
    assert!(eventually(|| running_copies(&exe).len() == 1), "unlocked program did not open");
    std::thread::sleep(Duration::from_secs(1));
    let opened = running_copies(&exe);
    assert_eq!(opened.len(), 1, "unlocked program was stopped");

    // Quit, then launched again: locked again.
    unsafe { libc::kill(opened[0] as i32, libc::SIGTERM) };
    assert!(eventually(|| running_copies(&exe).is_empty()));
    std::thread::sleep(Duration::from_millis(700));
    let mut again = Command::new(&exe).arg("30").spawn().unwrap();
    assert!(eventually(|| exited(&mut again)), "program was not locked again after quitting");
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), app);

    std::fs::remove_dir_all(&dir).unwrap();
}
