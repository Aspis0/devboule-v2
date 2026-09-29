//! A real daemon and its own log file (`daemon.log`, in the runtime dir).
//!
//! The app spawns the daemon with stderr on `Stdio::null()`, so every
//! `eprintln!` the daemon makes was going nowhere. These tests pin the sink:
//! a startup quarantine notice lands in `daemon.log`, an over-cap log is
//! rotated to `daemon.log.1` once the lock is held (and never by a losing
//! second daemon), and a path squatted by a directory neither stops the
//! daemon nor stays silent (`Status.logError` and the diagnostics report).
//!
//! Each test spawns the real binary and reads only files and the wire, so
//! each one fails on a tree without the sink: there is no `daemon.log` and
//! no `logError` in `Status`.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use devboule_daemon::{
    connect, current_user_sid, spawn_daemon_with_env, DaemonClient, RuntimePaths,
};
use devboule_protocol::{ClientHello, DaemonMessage, OwnerId};

fn daemon_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_devboule-daemon"))
}

fn unique_paths() -> (RuntimePaths, PathBuf) {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "devboule log {}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("runtime dir");
    (RuntimePaths::from_dir(&dir), dir)
}

/// The daemon is spawned with the **file** secret store rooted in its own
/// runtime dir: a production daemon otherwise writes a Noise key into the
/// Windows credential store, which nothing in a test run ever deletes.
fn spawn(paths: &RuntimePaths) -> ChildGuard {
    ChildGuard {
        child: spawn_daemon_with_env(&daemon_bin(), paths, &[("DEVBOULE_SECRET_STORE", "file")])
            .expect("spawn daemon"),
    }
}

struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    fn kill_and_wait(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Block until the daemon leaves on its own — with a timeout, so a hung
    /// shutdown fails the test instead of hanging the suite (the lock-loser
    /// path exits immediately; only a broken shutdown could not).
    fn wait_for_exit(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().expect("wait for the losing daemon") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the losing daemon did not exit within 10s"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn died(&mut self) -> bool {
        self.child.try_wait().expect("probe daemon").is_some()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
}

fn test_hello() -> ClientHello {
    let owner = OwnerId::new(
        current_user_sid().expect("sid"),
        format!("daemon-log-test-{}", std::process::id()),
    )
    .expect("owner");
    ClientHello::m3a(owner, "devboule-test")
}

/// Poll-connect until the daemon serves, as `acp_sessions.rs` does.
fn wait_until_up(paths: &RuntimePaths) -> DaemonClient {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Ok(client) = connect(paths, test_hello()) {
            return client;
        }
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Poll the log until `until` holds on its content, or the daemon dies first.
fn wait_for_log(path: &Path, process: &mut ChildGuard, until: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut content = String::new();
    while Instant::now() < deadline {
        if let Ok(bytes) = std::fs::read(path) {
            content = String::from_utf8_lossy(&bytes).into_owned();
            if until(&content) {
                return content;
            }
        }
        assert!(!process.died(), "the daemon exited before writing {path:?}");
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("{path:?} never held the expected content; last read: {content:?}");
}

/// One corrupted `tool-policies.json` is the guaranteed startup notice: the
/// store quarantines the file and says so on stderr.
fn seed_corrupt_policy(dir: &Path) {
    std::fs::write(dir.join("tool-policies.json"), b"{ not json").expect("seed policy");
}

#[test]
fn a_quarantine_notice_lands_in_daemon_log() {
    let (paths, dir) = unique_paths();
    seed_corrupt_policy(&dir);
    let mut process = spawn(&paths);
    let log = wait_for_log(&dir.join("daemon.log"), &mut process, |content| {
        content.contains("is unusable")
    });
    process.kill_and_wait();
    assert!(
        log.contains("tool policy"),
        "the notice must name its store: {log:?}"
    );
}

#[test]
fn a_daemon_log_over_the_cap_is_rotated_aside_at_startup() {
    let (paths, dir) = unique_paths();
    const LINE: &[u8] = b"0123456789abcde\n";
    // One line past the 5 MiB cap.
    let previous = LINE.repeat(327_681);
    std::fs::write(dir.join("daemon.log"), &previous).expect("seed the over-cap log");
    std::fs::write(dir.join("daemon.log.1"), b"stale").expect("seed the previous rotation");
    seed_corrupt_policy(&dir);

    let mut process = spawn(&paths);
    // The fresh daemon.log carries the new run's notice...
    let live = wait_for_log(&dir.join("daemon.log"), &mut process, |content| {
        content.contains("is unusable")
    });
    process.kill_and_wait();

    // ...and the old bytes sit in daemon.log.1, replacing the stale one.
    let rotated =
        std::fs::read(dir.join("daemon.log.1")).expect("daemon.log.1 must exist after rotation");
    assert_eq!(
        rotated, previous,
        "the over-cap log must be moved aside whole"
    );
    assert!(
        !live.contains("0123456789abcde"),
        "the fresh daemon.log must start empty, not keep the old bytes: {live:?}"
    );
}

#[test]
fn a_directory_squatting_on_daemon_log_does_not_stop_the_daemon() {
    let (paths, dir) = unique_paths();
    std::fs::create_dir(dir.join("daemon.log")).expect("squat the log path");
    let mut process = spawn(&paths);
    let client = wait_until_up(&paths);
    let body = client.status().expect("the daemon must still serve Status");
    let report = client
        .daemon_diagnostics()
        .expect("the daemon must still serve Diagnostics");
    process.kill_and_wait();

    let json =
        serde_json::to_value(DaemonMessage::Status { id: 0, body }).expect("serialize the status");
    let why = json
        .get("logError")
        .expect("Status must name the log failure (logError)")
        .as_str()
        .expect("logError is a string");
    assert!(
        why.contains("daemon.log"),
        "the reason must name the file it could not open: {why:?}"
    );
    assert!(
        !why.contains('\\') && !why.contains('/'),
        "the reason must not carry the absolute path: {why:?}"
    );

    // The diagnostics report is the artefact a user pastes into a bug
    // report: the log failure must be visible there too, redacted.
    let report_json = serde_json::to_value(&report).expect("serialize the report");
    let report_why = report_json["health"]["logError"]
        .as_str()
        .expect("the report's health section must carry logError");
    assert!(
        report_why.contains("daemon.log"),
        "the report must name the log: {report_why:?}"
    );
}

#[test]
fn a_losing_second_daemon_does_not_rotate_the_live_log() {
    let (paths, dir) = unique_paths();
    const LINE: &[u8] = b"0123456789abcde\n";
    let previous = LINE.repeat(327_681);
    std::fs::write(dir.join("daemon.log"), &previous).expect("seed the live log");

    // A daemon already owns this runtime dir; the spawned one must lose.
    let lock =
        devboule_daemon::SingleInstanceLock::acquire(&paths).expect("the test holds the lock");
    let mut process = spawn(&paths);
    let status = process.wait_for_exit();
    drop(lock);

    assert!(
        !dir.join("daemon.log.1").exists(),
        "a losing daemon must never rotate the running daemon's log"
    );
    let live = std::fs::read(dir.join("daemon.log")).expect("the live log must stay");
    assert!(
        live.starts_with(&previous),
        "the running daemon's log must keep its head; got {} bytes",
        live.len()
    );
    assert!(status.success(), "a second daemon exits 0: {status}");
}

/// Rotation is a lock-holder's startup action, not a side effect of the
/// first stderr line: a healthy daemon that never prints anything must
/// still rotate an over-cap log. On a tree that rotates on the reader's
/// first wake, this test fails — nothing ever wakes the reader.
#[test]
fn a_daemon_that_logs_nothing_still_rotates_once_it_owns_the_lock() {
    let (paths, dir) = unique_paths();
    const LINE: &[u8] = b"0123456789abcde\n";
    let previous = LINE.repeat(327_681);
    std::fs::write(dir.join("daemon.log"), &previous).expect("seed the over-cap log");
    // No corrupt policy, no sweep: nothing is guaranteed to print.

    let mut process = spawn(&paths);
    let rotated_path = dir.join("daemon.log.1");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if rotated_path.exists() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "an over-cap log must be rotated once the lock is held, without waiting for a line"
        );
        assert!(
            !process.died(),
            "the daemon exited before rotating {rotated_path:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    process.kill_and_wait();
    assert_eq!(
        std::fs::read(&rotated_path).expect("rotated aside"),
        previous,
        "the over-cap log must be moved aside whole"
    );
}
