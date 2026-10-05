//! What the process manager promises without a browser: flags, backoff, port,
//! endpoint ownership and lifecycle of the one child.

use std::io::Read;
use std::path::Path;
use std::process::{ChildStdout, Command, Stdio};
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::browser::cdp_ws::fake::{FakeServer, Step};

#[test]
fn the_launch_never_drops_the_sandbox() {
    for headless in [false, true] {
        let args = chrome_args(Path::new("/profile"), headless);
        assert!(
            !args.iter().any(|arg| arg == "--no-sandbox"),
            "sandbox must stay on: {args:?}"
        );
    }
}

#[test]
fn the_launch_always_carries_ownership_and_safety_flags() {
    let args = chrome_args(Path::new("/profile"), false);
    assert!(args.contains(&"--remote-debugging-port=0".to_owned()));
    assert!(args.contains(&"--no-first-run".to_owned()));
    assert!(args.contains(&"--no-default-browser-check".to_owned()));
    assert!(args.contains(&"about:blank".to_owned()));
    assert!(
        args.iter().any(|arg| arg == "--user-data-dir=/profile"),
        "profile must be the app's: {args:?}"
    );
}

#[test]
fn headless_is_a_test_flag_only() {
    let headed = chrome_args(Path::new("/p"), false);
    let headless = chrome_args(Path::new("/p"), true);
    assert!(!headed.iter().any(|arg| arg.contains("headless")));
    assert!(headless.contains(&"--headless=new".to_owned()));
}

#[test]
fn the_backoff_doubles_and_caps() {
    assert_eq!(restart_delay(0), Duration::from_secs(1));
    assert_eq!(restart_delay(1), Duration::from_secs(2));
    assert_eq!(restart_delay(2), Duration::from_secs(4));
    assert_eq!(restart_delay(3), Duration::from_secs(8));
    assert_eq!(restart_delay(4), Duration::from_secs(16));
    assert_eq!(restart_delay(5), Duration::from_secs(30));
    assert_eq!(restart_delay(6), Duration::from_secs(30));
    assert_eq!(restart_delay(100), Duration::from_secs(30));
}

/// A loopback debugger that answers the ownership probe with `pid` as its
/// `browser` process.
async fn endpoint_naming(pid: u64) -> FakeServer {
    FakeServer::start(vec![Step::Answer {
        result: json!({ "processInfo": [
            { "type": "renderer", "id": 7 },
            { "type": "browser", "id": pid },
        ]}),
    }])
    .await
}

#[test]
fn a_debugger_that_names_another_process_is_refused_without_a_command() {
    tauri::async_runtime::block_on(async {
        let server = endpoint_naming(4242).await;
        let refused = verify_owner(server.url(), 1234).await;
        let Err(text) = refused else {
            panic!("another process's browser must be refused: {refused:?}");
        };
        assert!(text.contains("4242"), "the refusal names the pid: {text}");
        assert_eq!(
            server.read().await,
            vec!["SystemInfo.getProcessInfo".to_owned()],
            "nothing but the probe is sent to a debugger that is not ours"
        );
    });
}

#[test]
fn a_debugger_that_names_this_process_is_ours() {
    tauri::async_runtime::block_on(async {
        let server = endpoint_naming(1234).await;
        verify_owner(server.url(), 1234)
            .await
            .expect("the pid matches");
        assert_eq!(
            server.read().await,
            vec!["SystemInfo.getProcessInfo".to_owned()]
        );
    });
}

#[test]
fn a_second_launch_on_one_profile_is_refused_before_any_child() {
    tauri::async_runtime::block_on(async {
        let app_data = tempfile::tempdir().expect("an app-data dir");
        let profile = app_data.path().join("browser-profile");
        std::fs::create_dir_all(&profile).expect("the profile dir");
        let owner = ResourceLock::acquire(&profile.join("devboule-browser.lock"))
            .expect("the running instance holds the profile");
        let missing_exe = app_data.path().join("no-such-chrome");
        let refused = CftBrowser::launch(&missing_exe, app_data.path(), true).await;
        let Err(CftError::Launch(text)) = refused else {
            panic!("a second browser on one profile must be refused");
        };
        assert!(
            text.contains("another Devboule"),
            "the refusal must say whose profile it is: {text}"
        );
        drop(owner);
    });
}

/// A real process that outlives the test unless its owner kills it, and that
/// spawns nothing of its own: a grandchild would inherit the pipe and keep it
/// open after the direct child died, which is exactly what the test reads.
fn long_lived() -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 30",
        ]);
        command
    }
    #[cfg(unix)]
    {
        let mut command = Command::new("sleep");
        command.arg("30");
        command
    }
}

/// The child's stdout pipe ends when the child is gone, so a read is proof it
/// was killed and reaped. The deadline fails the test instead of hanging it.
fn expect_eof(mut out: ChildStdout) {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut byte = [0u8; 1];
        let _ = sender.send(out.read(&mut byte));
    });
    let read = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("the child is gone");
    assert!(
        matches!(read, Ok(0)),
        "the pipe ends when the child is gone"
    );
}

#[test]
fn dropping_an_unstopped_browser_reaps_its_child() {
    let dir = tempfile::tempdir().expect("a scratch dir");
    let lock = ResourceLock::acquire(&dir.path().join("owner.lock")).expect("a lock");
    let mut command = long_lived();
    command.stdout(Stdio::piped());
    let mut child = command.spawn().expect("a long-lived child");
    let out = child.stdout.take().expect("the pipe");
    drop(CftBrowser::for_test(child, lock));
    expect_eof(out);
}

#[test]
fn shutdown_is_one_shot_a_finished_child_leaves_nothing_to_reap() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let lock = ResourceLock::acquire(&dir.path().join("owner.lock")).expect("a lock");
        let mut command = long_lived();
        command.stdout(Stdio::piped());
        let mut child = command.spawn().expect("a child");
        let out = child.stdout.take().expect("the pipe");
        child.kill().expect("the test ends it");
        let _ = child.wait();
        CftBrowser::for_test(child, lock).shutdown().await;
        expect_eof(out);
    });
}
