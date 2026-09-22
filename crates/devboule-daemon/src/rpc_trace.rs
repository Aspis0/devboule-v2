//! The env-gated request trace: it names the command a caller is waiting on
//! (app side) and the command the serving loop is dispatching (daemon side),
//! so a frozen window can be attributed to a command instead of a guess.
//!
//! `DEVBOULE_RPC_TRACE` reads as two kinds of value:
//!
//! - a **word** (no path separator) is a power switch: the allowlist
//!   `1`/`on`/`true`/`yes` (trimmed, case-insensitive) means the runtime
//!   directory, and **every other word is off and creates nothing** — `Off`
//!   is off, never a directory;
//! - a value **with a path separator** is the sink directory itself, created
//!   on first write, so a measurement run can aim both logs at its proof
//!   folder.
//!
//! Both sides write a **file**: the app spawns the daemon with stdout and
//! stderr on `Stdio::null()` (`spawn.rs`), so a daemon `eprintln!` is lost.
//! The two sides never share a file — the app does not link `daemon_event` —
//! and each file is appended under a process-wide lock.
//!
//! **Growth**: append only, **capped at 16 MiB per file** ([`TRACE_CAP_BYTES`]).
//! Past the cap the trace declares itself stopped with one
//! `event=trace_capped` line and writes nothing more: a capped trace that
//! says so beats a full disk. A trace left on all day still grows up to that
//! cap — this is a measurement tool, not a default to leave on. No lock
//! spans processes: two daemon processes appending the same daemon log (an
//! orphan plus its replacement, or two test-spawned daemons under one path
//! value) can tear a line — the parser then fails on that line instead of
//! believing it.
//!
//! One line per event: `key=value` fields, no spaces in any value, core
//! fields in a fixed order (`side event t name id [conn]`, then the event's
//! own fields):
//!
//! ```text
//! rpc side=app event=start t=1769123456789 name=SessionsList id=12 thread=main tid=3 budget_ms=30000
//! rpc side=daemon event=arrival t=1769123456791 name=SessionsList id=12 conn=1
//! ```
//!
//! `t` is wall-clock epoch milliseconds so the two files and the
//! responsiveness probe of a measurement run share one clock.

use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devboule_protocol::DaemonMessage;

use crate::error::DaemonError;
use crate::paths::RuntimePaths;

pub(crate) const ENV_VAR: &str = "DEVBOULE_RPC_TRACE";
pub(crate) const APP_FILE: &str = "rpc-trace-app.log";
/// Only the daemon side writes this file, and only a `server` build contains
/// that side — without the feature the constant would be dead code in the
/// client-only build the app produces.
#[cfg(feature = "server")]
pub(crate) const DAEMON_FILE: &str = "rpc-trace-daemon.log";
/// A file at this size gets one `trace_capped` line and nothing further.
const TRACE_CAP_BYTES: u64 = 16 * 1024 * 1024;
/// The one line the cap writes, and the marker [`already_capped`] looks for.
const CAPPED_EVENT: &str = "trace_capped";

/// One writer at a time: threads appending the same file must not
/// interleave halves of a line.
static SINK_LOCK: Mutex<()> = Mutex::new(());

/// The directory the trace writes into, or `None` while the trace is off.
///
/// A word without a path separator is a power switch: the allowlist
/// (`1`/`on`/`true`/`yes`, trimmed and case-insensitive) means the runtime
/// directory, every other word — `Off`, `0`, `""`, `proof` — means off and
/// creates nothing. A value carrying a path separator is an explicit sink
/// directory (`record` creates it on first write, because asking for a path
/// is asking for exactly that).
pub(crate) fn sink_dir() -> Option<PathBuf> {
    let value = std::env::var(ENV_VAR).ok()?;
    let trimmed = value.trim();
    if trimmed.contains('\\') || trimmed.contains('/') {
        return Some(PathBuf::from(trimmed));
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "1" | "on" | "true" | "yes" => RuntimePaths::from_env().ok().map(|paths| paths.dir),
        _ => None,
    }
}

pub(crate) fn enabled() -> bool {
    sink_dir().is_some()
}

/// Wall-clock milliseconds since the epoch — the one clock the app log, the
/// daemon log and a probe all share.
fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// One app-side event (`roundtrip_with_deadline` is the only caller today).
pub(crate) fn app_event(event: &str, name: &'static str, id: Option<u64>, fields: &[(&str, &str)]) {
    record("app", APP_FILE, event, name, id, None, fields);
}

/// One daemon-side event: arrival in the reader, dispatch window, reply.
/// Only a `server` build has that side; the client-only build must not carry
/// this as dead code.
#[cfg(feature = "server")]
pub(crate) fn daemon_event(
    event: &str,
    name: &'static str,
    id: Option<u64>,
    conn: u64,
    fields: &[(&str, &str)],
) {
    record("daemon", DAEMON_FILE, event, name, id, Some(conn), fields);
}

fn record(
    side: &'static str,
    file: &'static str,
    event: &str,
    name: &'static str,
    id: Option<u64>,
    conn: Option<u64>,
    fields: &[(&str, &str)],
) {
    let Some(dir) = sink_dir() else {
        return;
    };
    let id = id.map_or_else(|| "none".to_string(), |id| id.to_string());
    let mut line = format!(
        "rpc side={side} event={event} t={} name={name} id={id}",
        epoch_ms()
    );
    if let Some(conn) = conn {
        let _ = write!(line, " conn={conn}");
    }
    for (key, value) in fields {
        let _ = write!(line, " {key}={}", sanitize(value));
    }
    let _guard = SINK_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(file);
    let Ok(mut handle) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let size = handle
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if size >= TRACE_CAP_BYTES {
        // Past the cap the trace declares itself stopped — once per file, so
        // a second process sharing it does not repeat the declaration.
        if !already_capped(&path) {
            let _ = writeln!(
                handle,
                "rpc side={side} event={CAPPED_EVENT} t={} cap_bytes={TRACE_CAP_BYTES} file_bytes={size}",
                epoch_ms()
            );
        }
        return;
    }
    let _ = writeln!(handle, "{line}");
}

/// Whether the file already ends with the cap's stop line — read from the
/// file itself, so the decision holds across processes (two daemons on one
/// file) without a shared lock.
fn already_capped(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    // Longer than the stop line, so its marker is inside the window even
    // when the tail starts mid-line.
    const TAIL_BYTES: u64 = 256;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let len = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    if file
        .seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .is_err()
    {
        return false;
    }
    let mut tail = Vec::new();
    if file.read_to_end(&mut tail).is_err() {
        return false;
    }
    String::from_utf8_lossy(&tail).contains(CAPPED_EVENT)
}

/// A field value must stay one token: the line is parsed by splitting on spaces.
fn sanitize(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join("-")
}

/// The calling thread's numeric id. `ThreadId` has no stable numeric
/// accessor; `Debug` prints `ThreadId(3)` and the prefix is stripped when
/// that spelling holds (the raw rendering stays a valid value if it changes).
fn thread_id() -> String {
    let rendered = format!("{:?}", std::thread::current().id());
    rendered
        .strip_prefix("ThreadId(")
        .and_then(|rest| rest.strip_suffix(')'))
        .map(str::to_owned)
        .unwrap_or(rendered)
}

/// One traced roundtrip: `start` is written on the calling thread before the
/// request leaves, `done` when the reply — or the deadline — ends the wait.
pub(crate) struct Roundtrip {
    name: &'static str,
    id: Option<u64>,
    started: Instant,
    budget_ms: u128,
    recording: bool,
}

impl Roundtrip {
    pub(crate) fn begin(name: &'static str, id: Option<u64>, budget: Duration) -> Self {
        let recording = enabled();
        let started = Instant::now();
        let budget_ms = budget.as_millis();
        if recording {
            let thread = std::thread::current();
            let thread_name = sanitize(thread.name().unwrap_or("unnamed"));
            let tid = thread_id();
            app_event(
                "start",
                name,
                id,
                &[
                    ("thread", thread_name.as_str()),
                    ("tid", tid.as_str()),
                    ("budget_ms", &budget_ms.to_string()),
                ],
            );
        }
        Self {
            name,
            id,
            started,
            budget_ms,
            recording,
        }
    }

    pub(crate) fn finish(self, result: &Result<DaemonMessage, DaemonError>) {
        if !self.recording {
            return;
        }
        let status = match result {
            Ok(_) => "ok",
            Err(DaemonError::TimedOut(_)) => "timeout",
            Err(DaemonError::ConnectionLost) => "disconnected",
            Err(_) => "error",
        };
        let waited_ms = self.started.elapsed().as_millis();
        app_event(
            "done",
            self.name,
            self.id,
            &[
                ("waited_ms", &waited_ms.to_string()),
                ("budget_ms", &self.budget_ms.to_string()),
                ("status", status),
            ],
        );
    }
}

#[cfg(test)]
#[path = "rpc_trace_tests.rs"]
pub(crate) mod tests;
