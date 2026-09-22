//! Tests for the request trace: the env gate, the line format, and the
//! shared env guard.
//!
//! The variable is process-global and Rust runs tests in parallel, so every
//! test that touches it — here, in `connection_tests`, in `client_tests` —
//! holds the same lock for its whole body through [`trace_on`]/[`trace_off`].
//! One guard per test: the lock is not reentrant.

use super::*;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct TraceEnv {
    previous: Option<String>,
    _lock: MutexGuard<'static, ()>,
}

impl Drop for TraceEnv {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(ENV_VAR, value),
            None => std::env::remove_var(ENV_VAR),
        }
    }
}

/// Point the trace at `dir` until the returned guard is dropped.
pub(crate) fn trace_on(dir: &Path) -> TraceEnv {
    trace_set(Some(dir.to_str().expect("utf-8 scratch dir")))
}

/// The default state — the trace off — until the returned guard is dropped.
pub(crate) fn trace_off() -> TraceEnv {
    trace_set(None)
}

/// Lock the env and leave the variable unset: the caller then drives a whole
/// table of values through [`TraceEnv::put`] under this one guard (the lock
/// is not reentrant), and the previous value comes back on drop.
pub(crate) fn trace_held() -> TraceEnv {
    trace_set(None)
}

impl TraceEnv {
    /// Set the variable while this guard still holds the lock.
    pub(crate) fn put(&self, value: &str) {
        std::env::set_var(ENV_VAR, value);
    }
}

fn trace_set(value: Option<&str>) -> TraceEnv {
    let lock = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let previous = std::env::var(ENV_VAR).ok();
    match value {
        Some(value) => std::env::set_var(ENV_VAR, value),
        None => std::env::remove_var(ENV_VAR),
    }
    TraceEnv {
        previous,
        _lock: lock,
    }
}

/// A per-test directory under the system temp dir, keyed by label and pid so
/// two test binaries cannot collide.
pub(crate) fn scratch(label: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("devboule-rpc-trace-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

pub(crate) fn read_app_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(APP_FILE)).expect("app trace file")
}

/// Only a `server` build writes (and so reads) the daemon log: the client-only
/// build must not carry this as dead code either.
#[cfg(feature = "server")]
pub(crate) fn read_daemon_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(DAEMON_FILE)).expect("daemon trace file")
}

#[test]
fn the_trace_is_off_when_the_variable_is_unset() {
    let _env = trace_off();
    // `None` is the gate `record` returns on before it opens anything —
    // this is the default state of a process that was not prepared for a run.
    assert!(!enabled());
    assert!(sink_dir().is_none());
}

#[test]
fn a_path_value_names_the_sink_directory_and_a_line_lands_there() {
    let dir = scratch("path-value");
    let _env = trace_on(&dir);
    assert!(enabled());
    assert_eq!(sink_dir().as_deref(), Some(dir.as_path()));
    app_event("start", "Ping", Some(1), &[]);
    assert!(read_app_log(&dir).contains(" name=Ping id=1"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_short_value_points_at_the_runtime_directory() {
    let _env = trace_set(Some("1"));
    let expected = RuntimePaths::from_env().ok().map(|paths| paths.dir);
    assert_eq!(sink_dir(), expected);
    assert_eq!(enabled(), expected.is_some());
}

#[test]
fn a_line_is_stable_space_separated_key_values() {
    let dir = scratch("line-format");
    let _env = trace_on(&dir);
    app_event(
        "start",
        "SessionsList",
        Some(12),
        &[("thread", "main thread"), ("budget_ms", "30000")],
    );

    let log = read_app_log(&dir);
    // The sink is process-wide: roundtrips of tests running in parallel land
    // in this file too. This line is the one this test wrote (its field
    // "main-thread" is the sanitized marker it passed).
    let mine: Vec<&str> = log
        .lines()
        .filter(|line| line.contains(" thread=main-thread"))
        .collect();
    assert_eq!(mine.len(), 1, "{log}");
    let line = mine[0];
    let tokens: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(tokens.first().copied(), Some("rpc"), "{line}");
    let parsed: Vec<(&str, &str)> = tokens[1..]
        .iter()
        .map(|token| token.split_once('=').expect("key=value token"))
        .collect();
    let keys: Vec<&str> = parsed.iter().map(|(key, _)| *key).collect();
    // The core fields come first, in order; the event's own fields follow.
    assert_eq!(
        keys,
        ["side", "event", "t", "name", "id", "thread", "budget_ms"],
        "{line}"
    );
    assert_eq!(parsed[0].1, "app");
    assert_eq!(parsed[1].1, "start");
    parsed[2].1.parse::<u64>().expect("t is epoch ms");
    assert_eq!(parsed[3].1, "SessionsList");
    assert_eq!(parsed[4].1, "12");
    // A value with a space stays one token.
    assert_eq!(parsed[5].1, "main-thread");
    assert_eq!(parsed[6].1, "30000");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_request_without_an_id_is_still_a_named_line() {
    let dir = scratch("id-none");
    let _env = trace_on(&dir);
    app_event("start", "Hello", None, &[]);
    let line = read_app_log(&dir);
    assert!(line.contains(" name=Hello id=none"), "{line}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The value table: a word lights the trace up only on the allowlist
/// (trimmed, case-insensitive) and only as the runtime directory; every
/// other word is off; only a value with a path separator is a directory.
#[test]
fn only_the_allowlist_lights_up_and_only_a_path_is_a_directory() {
    let env = trace_held();
    let runtime = RuntimePaths::from_env().ok().map(|paths| paths.dir);

    for word in ["1", "on", "ON", "true", "TRUE", "yes", "Yes", "  on  "] {
        env.put(word);
        assert!(enabled(), "word {word:?} must light up");
        assert_eq!(sink_dir(), runtime, "word {word:?} means the runtime dir");
    }
    for word in [
        "0", "off", "Off", "OFF", "false", "False", "no", "", "   ", "proof", "nope",
    ] {
        env.put(word);
        assert!(!enabled(), "word {word:?} must stay off");
        assert!(sink_dir().is_none(), "word {word:?} is not a directory");
    }
    for dir in [
        r"C:\proof\trace",
        r"scout\freeze-followup\proof",
        "relative/proof",
    ] {
        env.put(dir);
        assert!(enabled(), "path {dir:?} is an explicit sink");
        assert_eq!(
            sink_dir().as_deref(),
            Some(Path::new(dir)),
            "path {dir:?} is itself the directory"
        );
    }
}

/// An off word creates nothing — not in the CWD (the old bug made `Off` a
/// directory there) and nowhere else: `record` returns before the
/// filesystem when the sink is `None`.
#[test]
fn an_off_word_creates_nothing_anywhere() {
    let env = trace_held();
    for word in ["Off", "0", "proof"] {
        env.put(word);
        assert!(sink_dir().is_none(), "word {word:?} must be off");
        app_event("start", "Ping", Some(1), &[]);
        assert!(
            !Path::new(word).exists(),
            "no directory named after the off word {word:?} in CWD"
        );
    }
    assert!(!Path::new(APP_FILE).exists(), "no log file in CWD either");
}

/// Past the cap the trace stops itself: one line that says so, and nothing
/// after it — a capped, declared trace instead of a full disk.
#[test]
fn the_trace_stops_at_the_cap_with_one_line_that_says_so() {
    let dir = scratch("cap");
    let _env = trace_on(&dir);
    let app_log = dir.join(APP_FILE);
    std::fs::File::create(&app_log)
        .expect("cap file")
        .set_len(TRACE_CAP_BYTES)
        .expect("fill the file to the cap");

    app_event("start", "Ping", Some(1), &[("thread", "main")]);
    app_event("done", "Ping", Some(1), &[("status", "ok")]);

    let log = read_app_log(&dir);
    assert!(log.contains("rpc side=app event=trace_capped"), "{log}");
    assert_eq!(log.matches("event=trace_capped").count(), 1, "{log}");
    assert!(log.contains("cap_bytes=16777216"), "{log}");
    assert!(!log.contains("event=start"), "no event past the cap: {log}");
    assert!(!log.contains("event=done"), "no event past the cap: {log}");
    let _ = std::fs::remove_dir_all(&dir);
}
