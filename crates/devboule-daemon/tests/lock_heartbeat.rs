//! A real daemon, a real lock file, and a probe that is not a connection.
//!
//! `ugly_paths.rs` proves the pipe; this proves the document beside it. The
//! test here is the one the lock range stands on: the record is read by *this*
//! process from a file the daemon holds locked, while that daemon is alive.
//! Put the lock back on the whole file and the read fails with
//! `ERROR_LOCK_VIOLATION`, the probe sees nothing, and it goes red.

#![cfg(windows)]

use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use devboule_daemon::{spawn_daemon_with_env, DaemonState, RuntimePaths};

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
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
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
