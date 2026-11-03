//! The Windows app lock, end to end: a locked program is launched and the watcher stops it. The state
//! machine is unit-tested in src/applock.rs; this is the part that only a real process can show.
#![cfg(windows)]

use deskmonk::applock::Watcher;
use std::sync::mpsc::channel;
use std::time::Duration;

#[test]
fn a_locked_program_is_stopped_when_it_is_launched() {
    let dir = std::env::temp_dir().join(format!("deskmonk-lock-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A uniquely named copy of a harmless program, so nothing the user is running shares its name.
    // cmd.exe, because the Windows 11 notepad and winver in System32 are stubs that hand over to a
    // packaged app and exit at once — nothing stays running for the watcher to find.
    let app = dir.join("dmlocktest.exe");
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    std::fs::copy(format!(r"{system}\System32\cmd.exe"), &app).unwrap();

    let (tx, rx) = channel();
    let _watcher = Watcher::start(vec![app.clone()], false, move |_, blocked| {
        let _ = tx.send(blocked);
    });

    // Something that stays running until it is stopped, which is the whole point of the test. "/k" would
    // not do: with no console to read from, cmd sees EOF at once and exits.
    let mut child = std::process::Command::new(&app)
        .args(["/c", "ping -n 60 127.0.0.1 >nul"])
        .stdin(std::process::Stdio::null())
        .spawn()
        .expect("the copy starts");
    let asked = rx.recv_timeout(Duration::from_secs(15)).expect("the watcher asks for the password");
    assert_eq!(asked, app, "the app that was stopped is the locked one");

    let status = child.wait().expect("it ended");
    assert!(!status.success(), "it was stopped, not closed by itself: {status:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
