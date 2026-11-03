//! Keeps App Lock in force while deskMonk's window is not open: while apps are locked it starts at login (a LaunchAgent on
//! macOS, an XDG autostart entry on Linux) and a guard process starts it again when it was stopped without the password.
//! Only one copy enforces the locks. Not a security boundary: whoever can run commands as the user can remove both.

use deskmonk::applock;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

/// Started at login or by the guard: no window in front (hidden on macOS, minimized on Linux).
pub const BACKGROUND: &str = "--background";
/// `deskmonk --guard PID`: waits for deskMonk PID to end, and starts it again if it was stopped while apps are locked.
pub const GUARD: &str = "--guard";

/// The program to start again: the AppImage itself, as the folder it runs from goes away when it quits.
fn exe() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE").filter(|_| cfg!(target_os = "linux")).map(PathBuf::from).or_else(|| std::env::current_exe().ok())
}

#[cfg(target_os = "macos")]
fn entry_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join("Library/LaunchAgents/com.pelamx.deskmonk.applock.plist"))
}

#[cfg(target_os = "linux")]
fn entry_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("autostart/deskmonk-applock.desktop"))
}

/// The login entry for `exe`; None when it runs from somewhere it won't be at the next login (the disk image).
#[cfg(target_os = "macos")]
fn entry(exe: &str) -> Option<String> {
    if exe.starts_with("/Volumes/") || exe.contains("/AppTranslocation/") {
        return None;
    }
    let exe = exe.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    Some(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key><string>com.pelamx.deskmonk.applock</string>
	<key>ProgramArguments</key><array><string>{exe}</string><string>{BACKGROUND}</string></array>
	<key>RunAtLoad</key><true/>
	<key>LimitLoadToSessionType</key><string>Aqua</string>
	<key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#
    ))
}

#[cfg(target_os = "linux")]
fn entry(exe: &str) -> Option<String> {
    // Desktop Entry spec: a quoted Exec argument escapes these with a backslash.
    let quoted: String = exe.chars().flat_map(|c| if "\\\"`$".contains(c) { vec!['\\', c] } else { vec![c] }).collect();
    Some(format!(
        "[Desktop Entry]\nType=Application\nName=deskMonk\nComment=Keeps App Lock on\nExec=\"{quoted}\" {BACKGROUND}\nIcon=deskmonk\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    ))
}

/// Starts deskMonk at login while apps are locked (`active`), and not when they are not.
pub fn sync(active: bool) {
    let Some(path) = entry_path() else { return };
    let result = if active {
        let Some(text) = exe().and_then(|e| entry(&e.to_string_lossy())) else { return };
        if fs::read_to_string(&path).is_ok_and(|old| old == text) {
            return;
        }
        fs::create_dir_all(path.parent().unwrap()).and_then(|()| fs::write(&path, text))
    } else {
        fs::remove_file(&path).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
    };
    if let Err(e) = result {
        eprintln!("login entry {}: {e}", path.display());
    }
}

/// True for the one copy that enforces the locks; a second window (Linux has no single-app rule) leaves it to the first.
/// The lock goes with the process, also when it is killed.
pub fn first_copy() -> bool {
    use std::os::fd::AsRawFd;
    let Some(dir) = deskmonk::config_dir() else { return true };
    let _ = fs::create_dir_all(&dir);
    let Ok(file) = fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("applock.lock")) else { return true };
    // A copy restarting itself into an update lets go first (hand_over).
    let first = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if first {
        LOCK.set(file).ok();
    }
    first
}

static LOCK: std::sync::OnceLock<fs::File> = std::sync::OnceLock::new();

/// Before restarting into an update: the next copy takes over the locks, and the guard does not start this one again.
pub fn hand_over() {
    if let Some(file) = LOCK.get() {
        use std::os::fd::AsRawFd;
        applock::clear_running();
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// Starts the guard for this copy.
pub fn spawn_guard() -> Option<std::process::Child> {
    let child = std::process::Command::new(exe()?)
        .arg(GUARD)
        .arg(std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn();
    child.map_err(|e| eprintln!("guard: {e}")).ok()
}

/// The guard: once deskMonk `pid` has ended, starts it again if it was stopped while apps are locked.
pub fn guard(pid: i32) -> ! {
    // shortcut: polls once a second, and a pid reused that fast keeps it waiting; harmless.
    while unsafe { libc::kill(pid, 0) } == 0 {
        std::thread::sleep(Duration::from_secs(1));
    }
    // At logout the session ends this process too, within this pause.
    std::thread::sleep(Duration::from_secs(2));
    if applock::stopped_while_locked()
        && let Some(exe) = exe()
    {
        let _ = std::process::Command::new(exe).arg(BACKGROUND).stdin(std::process::Stdio::null()).spawn();
    }
    std::process::exit(0)
}

/// macOS: the quit comes from logging out, restarting or shutting down (the quit Apple Event names a reason),
/// not from Cmd+Q or the Dock. Then the guard must not start deskMonk again.
#[cfg(target_os = "macos")]
pub fn system_quit() -> bool {
    use objc2_foundation::NSAppleEventManager;
    // keyAEQuitReason, 'why?'
    const QUIT_REASON: u32 = u32::from_be_bytes(*b"why?");
    let event = NSAppleEventManager::sharedAppleEventManager().currentAppleEvent();
    event.is_some_and(|e| {
        let reason: Option<objc2::rc::Retained<objc2::runtime::AnyObject>> = unsafe { objc2::msg_send![&*e, attributeDescriptorForKeyword: QUIT_REASON] };
        reason.is_some()
    })
}

#[cfg(target_os = "linux")]
pub fn system_quit() -> bool {
    false
}
