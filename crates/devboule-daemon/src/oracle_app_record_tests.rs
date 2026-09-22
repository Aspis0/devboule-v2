//! Tests for the record the app publishes. The stale case carries this
//! process's own pid — alive by construction — so a reader that consulted
//! the pid instead of the mtime would answer `Live` and die here.

use super::*;

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::daemon_record::{Heartbeat, RECORD_CAPACITY, STALE_AFTER};
use crate::error::DaemonError;
use crate::lock::SingleInstanceLock;

fn unique_dir() -> (PathBuf, DirGuard) {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "devboule oracle app record {}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let guard = DirGuard(dir.clone());
    (dir, guard)
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Move the record's modification time into the past, which is the only
/// thing a beat ever changes.
fn age(path: &Path, by: Duration) {
    let file = OpenOptions::new().write(true).open(path).expect("open");
    file.set_modified(SystemTime::now() - by).expect("age");
}

fn running() -> OracleAppRecord {
    let mut record = OracleAppRecord::new(std::process::id(), "instance-1", 50_000, "deadbeef");
    record.listening();
    record
}

#[test]
fn a_record_round_trips_through_its_body() {
    let mut record = OracleAppRecord::new(4321, "instance-1", 50_000, "deadbeef");
    assert!(!record.ready, "not ready before the bind");
    assert_eq!(OracleAppRecord::parse(&record.body()), Some(record.clone()));

    record.listening();
    assert!(record.ready, "ready after the bind");
    assert_eq!(OracleAppRecord::parse(&record.body()), Some(record.clone()));
    assert!(
        record.body().len() < RECORD_CAPACITY as usize,
        "the record has to fit in the bytes the lock does not cover"
    );

    assert!(OracleAppRecord::parse("").is_none());
    assert!(OracleAppRecord::parse("pid=1\ninstance=i\n").is_none());
    assert_eq!(
        OracleAppState::read(Path::new("no-such-oracle-app-record")),
        OracleAppState::Absent
    );
}

/// The negative control: the pid in the record is alive by construction, so
/// only the mtime can decide. 121 s is one second past [`STALE_AFTER`].
#[test]
fn a_record_of_121_seconds_is_stale_even_though_the_pid_in_it_is_alive() {
    assert_eq!(STALE_AFTER, Duration::from_secs(120));
    let (dir, _guard) = unique_dir();
    let path = dir.join(ORACLE_APP_LOCK_FILE_NAME);
    std::fs::write(&path, running().body()).expect("write");
    assert!(
        OracleAppState::read(&path).is_live(),
        "a record just written is live"
    );

    age(&path, Duration::from_secs(121));
    match OracleAppState::read(&path) {
        OracleAppState::Stale(record) => assert_eq!(
            record.pid,
            std::process::id(),
            "the reader saw the alive pid and still refused to believe the record"
        ),
        other => panic!("a 121-second-old record must be stale, got {other:?}"),
    }
}

/// A heartbeat that could create the file it beats on would publish an
/// endpoint that is not there, so it opens without `create` and refuses.
#[test]
fn the_heartbeat_refuses_to_beat_on_a_record_that_is_not_there() {
    let (dir, _guard) = unique_dir();
    let missing = dir.join(ORACLE_APP_LOCK_FILE_NAME);
    assert!(Heartbeat::with_interval(&missing, Duration::from_millis(10)).is_err());
}

#[test]
fn a_second_lock_on_the_same_record_path_is_refused() {
    let (dir, _guard) = unique_dir();
    let path = dir.join(ORACLE_APP_LOCK_FILE_NAME);
    let _first = SingleInstanceLock::acquire_at(&path).expect("first");
    match SingleInstanceLock::acquire_at(&path) {
        Err(DaemonError::AlreadyRunning) => {}
        Ok(_) => panic!("second lock succeeded"),
        Err(error) => panic!("expected AlreadyRunning, got {error}"),
    }
}

/// A body that reaches disk before the bind reads live — fresh mtime — but
/// must not answer "ready": readiness is what the reader of this record
/// gates a connection on.
#[test]
fn a_body_written_before_the_bind_reads_live_but_not_ready() {
    let (dir, _guard) = unique_dir();
    let path = dir.join(ORACLE_APP_LOCK_FILE_NAME);
    let unpublished = OracleAppRecord::new(std::process::id(), "instance-1", 50_000, "deadbeef");
    std::fs::write(&path, unpublished.body()).expect("write");
    let state = OracleAppState::read(&path);
    assert!(state.is_live(), "a fresh body is live");
    assert!(!state.is_ready(), "live without the bind must not be ready");

    std::fs::write(&path, running().body()).expect("write after the bind");
    assert!(
        OracleAppState::read(&path).is_ready(),
        "the body written after the bind is ready"
    );
}
