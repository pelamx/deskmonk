//! Keeps App Lock in force while deskMonk's window is not open: while apps are locked it starts at login (a LaunchAgent on
//! macOS, an XDG autostart entry on Linux, a per-user Run entry on Windows) and a guard process starts it again when it
//! was stopped without the password.
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
    std::env::var_os("APPIMAGE").filter(|_| cfg!(target_os = "linux")).map(PathBuf::from).or_else(|| Some(deskmonk::update::unold(&std::env::current_exe().ok()?)))
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

/// Windows keeps login items in the registry, not in a file. Per user and without admin rights, like
/// the LaunchAgent and the autostart entry: the user can take it out again in Task Manager > Startup.
/// `reg.exe` rather than the registry API: five lines instead of fifty of UTF-16 and handles, and it
/// runs rarely — only when the last app is locked or unlocked.
#[cfg(windows)]
pub fn sync(active: bool) {
    use std::os::windows::process::CommandExt;
    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const NAME: &str = "deskMonk App Lock";
    const NO_WINDOW: u32 = 0x0800_0000; // CREATE_NO_WINDOW: reg.exe must not flash a console
    let mut cmd = std::process::Command::new("reg.exe");
    if active {
        let Some(exe) = exe() else { return };
        // The value is one command line: the program is quoted, so a path with spaces still runs.
        cmd.args(["add", RUN_KEY, "/v", NAME, "/t", "REG_SZ", "/f", "/d"]).arg(format!("\"{}\" {BACKGROUND}", exe.display()));
    } else {
        cmd.args(["delete", RUN_KEY, "/v", NAME, "/f"]);
    }
    // Deleting a value that is not there answers with an error; that is the state we wanted anyway.
    if let Err(e) = cmd.creation_flags(NO_WINDOW).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status() {
        eprintln!("login entry: {e}");
    }
}

/// Starts deskMonk at login while apps are locked (`active`), and not when they are not.
#[cfg(not(windows))]
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
    let Some(dir) = deskmonk::config_dir() else { return true };
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("applock.lock");
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    // Windows has no flock: opening the file while sharing it with nobody is the same bargain, and
    // the OS drops it when the process ends, however it ends.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    let Ok(file) = options.open(&path) else { return cfg!(not(windows)) };
    // A copy restarting itself into an update lets go first (hand_over).
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return false;
        }
    }
    *LOCK.lock().unwrap() = Some(file);
    true
}

static LOCK: std::sync::Mutex<Option<fs::File>> = std::sync::Mutex::new(None);

/// Before restarting into an update: the next copy takes over the locks, and the guard does not start this one again.
pub fn hand_over() {
    if let Some(file) = LOCK.lock().unwrap().take() {
        applock::clear_running();
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        }
        // Windows releases it when the handle closes.
        drop(file);
    }
}

/// Another copy holds the locks: the one this copy restarted into after an update, which sets the marker again.
fn other_copy() -> bool {
    let Some(path) = deskmonk::config_dir().map(|d| d.join("applock.lock")) else { return false };
    // Windows: that copy opened it sharing with nobody (first_copy), so opening it fails while it runs.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        path.exists() && fs::OpenOptions::new().read(true).share_mode(0).open(&path).is_err()
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let Ok(file) = fs::File::open(&path) else { return false };
        let free = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        !free
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

/// Waits for deskMonk `pid` to end.
#[cfg(unix)]
fn wait_for(pid: i32) {
    // shortcut: polls once a second, and a pid reused that fast keeps it waiting; harmless.
    while unsafe { libc::kill(pid, 0) } == 0 {
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Windows hands out a handle that is signalled when the process ends, so there is nothing to poll
/// and no window in which a reused pid could be mistaken for the one being watched.
#[cfg(windows)]
fn wait_for(pid: i32) {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_FAILED};
    use windows_sys::Win32::System::Threading::{INFINITE, OpenProcess, WaitForSingleObject};
    // windows-sys keeps this one under its filesystem feature, which is a lot to pull in for a number.
    const SYNCHRONIZE: u32 = 0x0010_0000;
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid as u32) };
    if handle.is_null() {
        return; // already gone, or not ours to watch
    }
    if unsafe { WaitForSingleObject(handle, INFINITE) } == WAIT_FAILED {
        std::thread::sleep(Duration::from_secs(1));
    }
    unsafe { CloseHandle(handle) };
}

/// The guard: once deskMonk `pid` has ended, starts it again if it was stopped while apps are locked.
pub fn guard(pid: i32) -> ! {
    wait_for(pid);
    // At logout the session ends this process too, within this pause.
    std::thread::sleep(Duration::from_secs(2));
    if applock::stopped_while_locked()
        && !other_copy()
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

/// Linux and Windows: the guard is a child of this session, so logging out or shutting down ends it
/// too and nothing starts deskMonk again. Only a quit by hand has to be told apart, and it is not.
#[cfg(not(target_os = "macos"))]
pub fn system_quit() -> bool {
    false
}

/// The Windows pieces that have no counterpart anywhere else. The Run entry is left out on purpose:
/// a test that writes it would also delete a real one from the machine of whoever runs the suite.
#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn the_guard_waits_for_the_process_to_end() {
        let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let mut child = std::process::Command::new(format!(r"{system}\System32\cmd.exe"))
            .args(["/c", "ping -n 3 127.0.0.1 >nul"])
            .stdin(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let start = std::time::Instant::now();
        super::wait_for(pid);
        let waited = start.elapsed();
        let _ = child.wait();
        assert!(waited.as_millis() > 500, "returned before the process ended: {waited:?}");
        assert!(waited.as_secs() < 20, "did not notice it ended: {waited:?}");
    }
}
