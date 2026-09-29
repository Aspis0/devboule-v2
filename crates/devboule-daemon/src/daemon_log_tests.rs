//! The log's decision and its file: what stderr is taken over, when the file
//! rotates, and what the cap allows onto the disk. The pipeline's tests —
//! queue, drops, the bounded shutdown — live in `log_pipeline_tests.rs`.

use super::imp::*;
use std::fs::OpenOptions;
use windows_sys::Win32::Storage::FileSystem::{
    GetFileType, FILE_TYPE_CHAR, FILE_TYPE_DISK, FILE_TYPE_PIPE, FILE_TYPE_UNKNOWN,
};
use windows_sys::Win32::System::Console::{GetConsoleMode, CONSOLE_MODE};

#[test]
fn only_stderr_that_goes_nowhere_is_taken_over() {
    assert!(!stderr_going_nowhere(FILE_TYPE_DISK, false));
    assert!(!stderr_going_nowhere(FILE_TYPE_PIPE, false));
    assert!(!stderr_going_nowhere(FILE_TYPE_CHAR, true));
    assert!(stderr_going_nowhere(FILE_TYPE_CHAR, false));
    assert!(stderr_going_nowhere(FILE_TYPE_UNKNOWN, false));
}

#[test]
fn the_real_nul_device_reads_as_going_nowhere() {
    use std::os::windows::io::AsRawHandle as _;
    // The whole feature silently no-ops if a real `Stdio::null()`
    // handle ever reads as a console: pin the actual handle types.
    let nul = OpenOptions::new()
        .write(true)
        .open("NUL")
        .expect("the NUL device opens");
    let handle = nul.as_raw_handle();
    let file_type = unsafe { GetFileType(handle) };
    assert_eq!(file_type, FILE_TYPE_CHAR, "NUL is a character device");
    let mut mode: CONSOLE_MODE = 0;
    let is_console = unsafe { GetConsoleMode(handle, &mut mode) != 0 };
    assert!(!is_console, "NUL must not answer GetConsoleMode");
    assert!(stderr_going_nowhere(file_type, is_console));
}

#[test]
fn a_rotation_moves_an_over_cap_log_whole_and_replaces_the_old_one() {
    let dir = crate::test_dirs::test_temp_dir("devboule-rotate");
    let log_path = dir.join(LOG_FILE_NAME);
    let previous = vec![b'x'; (LOG_CAP_BYTES + 1) as usize];
    std::fs::write(&log_path, &previous).expect("seed the over-cap log");
    std::fs::write(dir.join(ROTATED_FILE_NAME), b"stale").expect("seed the old rotation");

    assert!(rotate_at_startup(&log_path));
    assert_eq!(
        std::fs::read(dir.join(ROTATED_FILE_NAME)).expect("rotated"),
        previous
    );
    assert!(
        !log_path.exists(),
        "the live name must be free for a fresh log"
    );
}

#[test]
fn a_log_at_or_under_the_cap_is_not_rotated() {
    let dir = crate::test_dirs::test_temp_dir("devboule-rotate-under");
    let log_path = dir.join(LOG_FILE_NAME);
    std::fs::write(&log_path, vec![b'x'; LOG_CAP_BYTES as usize]).expect("seed at cap");

    assert!(!rotate_at_startup(&log_path));
    assert_eq!(
        std::fs::metadata(&log_path).expect("still there").len(),
        LOG_CAP_BYTES
    );
}

#[test]
fn append_stops_at_the_cap_with_one_notice() {
    let dir = crate::test_dirs::test_temp_dir("devboule-append-cap");
    let log_path = dir.join(LOG_FILE_NAME);
    std::fs::write(&log_path, vec![b'x'; LOG_CAP_BYTES as usize]).expect("seed at cap");
    let file = OpenOptions::new()
        .append(true)
        .open(&log_path)
        .expect("open");
    let mut log = LogFile::appending(Some(file));

    log.append(b"one\n");
    log.append(b"two\n");

    let bytes = std::fs::read(&log_path).expect("read back");
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains("one"),
        "nothing is appended at the cap: {text:?}"
    );
    assert!(
        text.matches("cap of").count() == 1,
        "the cap notice is written exactly once: {text:?}"
    );
}

/// The cap notice is the only thing `append` writes past the cap, as it
/// trips. Every other note — drop tallies in particular — must keep the
/// high-water mark where it is.
#[test]
fn a_note_past_the_cap_writes_nothing() {
    let dir = crate::test_dirs::test_temp_dir("devboule-note-cap");
    let log_path = dir.join(LOG_FILE_NAME);
    std::fs::write(&log_path, vec![b'x'; LOG_CAP_BYTES as usize]).expect("seed at cap");
    let file = OpenOptions::new()
        .append(true)
        .open(&log_path)
        .expect("open");
    let mut log = LogFile::appending(Some(file));

    log.append(b"the line that trips the cap\n");
    log.note("daemon log: 7 stderr chunk(s) dropped while the log writer could not keep up");

    let bytes = std::fs::read(&log_path).expect("read back");
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains("chunk(s) dropped"),
        "a note must not grow the file past the cap: {text:?}"
    );
    assert!(
        text.contains("cap of"),
        "the cap's own notice still lands: {text:?}"
    );
}

#[test]
fn append_below_the_cap_writes_through() {
    let dir = crate::test_dirs::test_temp_dir("devboule-append");
    let log_path = dir.join(LOG_FILE_NAME);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .expect("open");
    let mut log = LogFile::appending(Some(file));

    log.append(b"hello\n");

    assert_eq!(std::fs::read(&log_path).expect("read back"), b"hello\n");
}

#[test]
fn append_without_a_file_is_silent() {
    let mut log = LogFile::appending(None);
    log.append(b"dropped\n");
    log.note("dropped note\n");
}

/// A reopen failure is recorded on the writer thread, after the state
/// exists: the status must read the error at request time, not snapshot it
/// at construction. On a tree that snapshots at construction this test
/// fails, because the field was copied before the record happened.
#[test]
fn status_carries_a_log_error_recorded_after_construction() {
    let dir = crate::test_dirs::test_temp_dir("devboule-log-error-late");
    let state = crate::server::ServerState::with_paths(
        "log-error-late".to_string(),
        crate::paths::RuntimePaths::from_dir(&dir),
    )
    .expect("the state builds for the temp runtime dir");
    record_error("daemon.log: test reopen failure".to_string());

    match state.status_body_for_test(1) {
        devboule_protocol::DaemonMessage::Status { body, .. } => assert_eq!(
            body.log_error.as_deref(),
            Some("daemon.log: test reopen failure"),
            "a log error recorded after construction must reach Status"
        ),
        other => panic!("expected a status frame, got {other:?}"),
    }
}
