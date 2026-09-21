//! Tests for the on-disk daemon record and the heartbeat that keeps it
//! believable. The rule under test has to *die* when the rule changes: the
//! aged-record case carries a pid that is alive by construction, so a reader
//! that asked the pid instead of the heartbeat answers `Live` there.

use super::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

fn unique_dir() -> (PathBuf, DirGuard) {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "devboule record {}-{}",
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

/// Move the record's modification time into the past, which is the only thing
/// a beat ever changes.
fn age(path: &Path, age: Duration) {
    let file = OpenOptions::new().write(true).open(path).expect("open");
    file.set_modified(SystemTime::now() - age).expect("age");
}

fn write_record(dir: &Path, record: &DaemonRecord) -> PathBuf {
    let path = dir.join("daemon.lock");
    std::fs::write(&path, record.body()).expect("write record");
    path
}

fn running(pid: u32) -> DaemonRecord {
    DaemonRecord::starting(pid, "instance-1", "\\\\.\\pipe\\devboule-test")
}

#[test]
fn a_record_round_trips_through_its_body() {
    let mut record = running(4321);
    assert_eq!(DaemonRecord::parse(&record.body()), Some(record.clone()));
    assert!(!DaemonState::read(Path::new("no-such-file")).is_live());
    assert!(!record.ready);

    record.listening();
    let parsed = DaemonRecord::parse(&record.body()).expect("record");
    assert!(parsed.ready, "the ready marker survives the round trip");
    assert_eq!(parsed.exit, None);

    record.stopped(ExitReason::Idle);
    let parsed = DaemonRecord::parse(&record.body()).expect("record");
    assert_eq!(parsed.exit, Some(ExitReason::Idle));
    assert!(!parsed.ready, "a stopped daemon is not listening");
}

#[test]
fn a_body_that_is_not_a_record_is_absent() {
    assert_eq!(DaemonRecord::parse(""), None);
    assert_eq!(DaemonRecord::parse("pid=1\n"), None, "no instance");
    assert_eq!(DaemonRecord::parse("instance=x\n"), None, "no pid");
    assert_eq!(DaemonRecord::parse("pid=nine\ninstance=x\n"), None);
    // Unknown keys are forward compatibility, not a parse failure: a later
    // version may add a field this build knows nothing about.
    let record = DaemonRecord::parse("pid=7\ninstance=7-1\nfuture=yes\n").expect("record");
    assert_eq!(record.instance_id, "7-1");

    let (dir, _guard) = unique_dir();
    let path = dir.join("daemon.lock");
    std::fs::write(&path, "not a record at all\n").expect("write");
    assert_eq!(DaemonState::read(&path), DaemonState::Absent);
    assert_eq!(
        DaemonState::read(&dir.join("missing.lock")),
        DaemonState::Absent
    );
}

#[test]
fn a_fresh_record_is_live_and_ready_only_after_the_listener_binds() {
    let (dir, _guard) = unique_dir();
    let mut record = running(std::process::id());
    let path = write_record(&dir, &record);

    let state = DaemonState::read(&path);
    assert!(state.is_live(), "a record just written is live: {state:?}");
    assert!(
        !state.is_ready(),
        "a daemon that has not bound its listener is live but not ready"
    );

    record.listening();
    std::fs::write(&path, record.body()).expect("write");
    assert!(DaemonState::read(&path).is_ready());
}

/// The negative control the whole rule stands on.
///
/// The pid in the record is **this test process**, so it is alive by
/// construction and provably so. A reader that decided by `is_pid_running`
/// would answer `Live` for an aged record; the assertion below turns red the
/// moment someone replaces the heartbeat with that rule, which is what the
/// file exists to stop.
#[test]
fn an_aged_record_is_stale_even_though_the_pid_in_it_is_alive() {
    let (dir, _guard) = unique_dir();
    let alive = std::process::id();
    let mut record = running(alive);
    record.listening();
    let path = write_record(&dir, &record);
    age(&path, STALE_AFTER + Duration::from_secs(1));

    match DaemonState::read(&path) {
        DaemonState::Stale(record) => assert_eq!(
            record.pid, alive,
            "the reader saw the alive pid and still refused to believe the record"
        ),
        other => panic!("an aged record must not be believed, got {other:?}"),
    }
}

#[test]
fn the_age_that_decides_is_the_beat_and_it_is_counted_in_beats() {
    let (dir, _guard) = unique_dir();
    let path = write_record(&dir, &running(1));

    age(&path, STALE_AFTER - Duration::from_secs(1));
    assert!(
        DaemonState::read(&path).is_live(),
        "one second inside the window is still inside it"
    );

    age(&path, STALE_AFTER + Duration::from_secs(1));
    assert!(matches!(DaemonState::read(&path), DaemonState::Stale(_)));

    assert_eq!(
        STALE_AFTER,
        HEARTBEAT_INTERVAL * STALE_BEATS,
        "the window is a whole number of beats, so the constants cannot drift apart"
    );
}

/// The reason outlives the process, which is the point of writing it down: an
/// old goodbye is a deliberate exit that happened long ago, not a crash.
#[test]
fn a_record_that_says_why_it_stopped_is_believed_however_old_it_is() {
    let (dir, _guard) = unique_dir();
    let mut record = running(999_999);
    record.stopped(ExitReason::Idle);
    let path = write_record(&dir, &record);
    age(&path, STALE_AFTER * 100);

    match DaemonState::read(&path) {
        DaemonState::Stopped(record, reason) => {
            assert_eq!(reason, ExitReason::Idle);
            assert_eq!(record.pid, 999_999, "the record still names who stopped");
            assert!(!DaemonState::read(&path).is_live());
        }
        other => panic!("a goodbye is not a crash, got {other:?}"),
    }
}

/// A reason this build does not know is still a goodbye: reading it as
/// `Requested` would put a word in a dead daemon's mouth.
#[test]
fn a_reason_from_a_later_version_is_still_a_deliberate_exit() {
    let (dir, _guard) = unique_dir();
    let path = dir.join("daemon.lock");
    std::fs::write(&path, "pid=1\ninstance=1-1\nexit=upgraded\n").expect("write");

    match DaemonState::read(&path) {
        DaemonState::Stopped(_, reason) => assert_eq!(reason, ExitReason::Unknown),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn the_heartbeat_makes_an_aged_record_believable_again() {
    let (dir, _guard) = unique_dir();
    let path = write_record(&dir, &running(std::process::id()));
    age(&path, STALE_AFTER + Duration::from_secs(1));
    assert!(matches!(DaemonState::read(&path), DaemonState::Stale(_)));

    let mut heartbeat =
        Heartbeat::with_interval(&path, Duration::from_millis(20)).expect("heartbeat");
    let deadline = Instant::now() + Duration::from_secs(2);
    while !DaemonState::read(&path).is_live() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        DaemonState::read(&path).is_live(),
        "a beating record is live again"
    );

    heartbeat.stop();
    age(&path, STALE_AFTER + Duration::from_secs(1));
    thread::sleep(Duration::from_millis(100));
    assert!(
        matches!(DaemonState::read(&path), DaemonState::Stale(_)),
        "a stopped heartbeat must not touch the record again"
    );
}

/// A heartbeat that could create the file it beats on would publish a daemon
/// that is not there, so it opens without `create` and refuses to start.
#[test]
fn a_heartbeat_refuses_to_beat_on_a_file_that_is_not_there() {
    let (dir, _guard) = unique_dir();
    let missing = dir.join("missing.lock");
    assert!(Heartbeat::with_interval(&missing, Duration::from_millis(10)).is_err());
    assert!(!missing.exists(), "and it did not create one");
}
