//! A real daemon, a real lock file, and a probe that is not a connection.
//!
//! `ugly_paths.rs` proves the pipe; this proves the document beside it. The
//! first test is the one the lock range stands on: the record is read by *this*
//! process from a file the daemon holds locked, while that daemon is alive.
//! Put the lock back on the whole file and the read fails with
//! `ERROR_LOCK_VIOLATION`, the probe sees nothing, and it goes red.
//!
//! The other two are the pair that makes `exit=` mean something: a daemon that
//! leaves on its own records why, and a daemon that is killed cannot.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use devboule_daemon::{
    connect, spawn_daemon_with_env, DaemonState, ExitReason, RuntimePaths, STALE_AFTER,
};
use devboule_protocol::{ClientHello, OwnerId};

fn daemon_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_devboule-daemon"))
}

fn unique_paths() -> (RuntimePaths, PathBuf) {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "devboule record {}-{}",
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

    /// Wait for the daemon to leave on its own, with the idle grace plus room.
    fn wait_until_exit(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().expect("wait for daemon") {
                return status;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("the daemon did not exit after becoming idle");
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
}

fn test_hello() -> ClientHello {
    let owner = OwnerId::new(
        devboule_daemon::current_user_sid().expect("sid"),
        format!("app-record-test-{}", std::process::id()),
    )
    .expect("owner");
    ClientHello::m3a(owner, "devboule-test")
}

/// Poll the record until it satisfies `until`, or the daemon dies first.
fn wait_for_record(
    paths: &RuntimePaths,
    process: &mut ChildGuard,
    until: impl Fn(&DaemonState) -> bool,
) -> DaemonState {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let state = DaemonState::read(&paths.lock_file);
        if until(&state) {
            return state;
        }
        if let Ok(Some(status)) = process.child.try_wait() {
            panic!("the daemon exited before its record was usable: {status}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("no usable record within 5s at {}", paths.dir.display());
}

fn age(path: &Path, age: Duration) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open record");
    file.set_modified(std::time::SystemTime::now() - age)
        .expect("age record");
}

#[test]
fn the_record_is_readable_from_another_process_while_the_daemon_holds_the_lock() {
    let (paths, dir) = unique_paths();
    let mut process = spawn(&paths);

    let state = wait_for_record(&paths, &mut process, |state| state.is_ready());
    let record = match state {
        DaemonState::Live(record) => record,
        other => panic!("the daemon is alive and listening, got {other:?}"),
    };
    assert_eq!(
        record.pid,
        process.child.id(),
        "the record names the process that wrote it"
    );
    assert_eq!(record.pipe_name, paths.pipe_name);
    assert!(
        record.instance_id.starts_with(&format!("{}-", record.pid)),
        "the instance id carries its own pid: {}",
        record.instance_id
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_daemon_that_leaves_on_its_own_records_why() {
    let (paths, dir) = unique_paths();
    let mut process = spawn(&paths);
    wait_for_record(&paths, &mut process, |state| state.is_ready());

    let client = connect(&paths, test_hello()).expect("connect");
    drop(client);
    let pid = process.child.id();
    let status = process.wait_until_exit();
    assert_eq!(status.code(), Some(0), "an idle exit is not a failure");

    match DaemonState::read(&paths.lock_file) {
        DaemonState::Stopped(record, reason) => {
            assert_eq!(reason, ExitReason::Idle);
            assert_eq!(record.pid, pid, "the record still names who left");
        }
        other => panic!("a deliberate exit has to say so, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_killed_daemon_leaves_no_goodbye_and_stops_being_believed() {
    let (paths, dir) = unique_paths();
    let mut process = spawn(&paths);
    wait_for_record(&paths, &mut process, |state| state.is_ready());

    process.kill_and_wait();
    assert!(
        !matches!(
            DaemonState::read(&paths.lock_file),
            DaemonState::Stopped(..)
        ),
        "a daemon that never ran its shutdown path cannot have said why it left"
    );

    age(&paths.lock_file, STALE_AFTER + Duration::from_secs(1));
    match DaemonState::read(&paths.lock_file) {
        DaemonState::Stale(record) => assert_eq!(
            record.pid,
            process.child.id(),
            "the residue is the record of the process that was killed"
        ),
        other => panic!("a record whose beats stopped is not believed, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}
