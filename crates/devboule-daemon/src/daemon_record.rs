//! The daemon's on-disk record: who it is, whether it is listening, how long
//! since it last proved it was alive, and why it stopped.
//!
//! The record lives in the single-instance lock file, the only artifact the
//! daemon owns outright, and it has to survive in a file another process
//! holds locked. Two uses pull in opposite directions: the lock must outlive a
//! process that dies without saying goodbye, and the record must be readable
//! *while* the lock is held. The second decides how the lock is taken — see
//! [`RECORD_CAPACITY`] and `lock::try_lock_exclusive` — because a byte-range
//! lock covering the record makes a daemon's own identity unreadable by every
//! other process for exactly as long as the identity matters.
//!
//! Liveness is published as the file's modification time, touched once per
//! [`HEARTBEAT_INTERVAL`]. The body is written three times in a process's life
//! (starting, listening, stopped) and never on a beat, so a reader can only
//! catch a torn record in those three instants. A record not touched for
//! [`STALE_BEATS`] intervals is not believed, and the pid inside it is *not*
//! consulted: a recycled pid is the failure this file exists to stop
//! repeating.

use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

/// The bytes at the head of the lock file the record is written into.
///
/// `SingleInstanceLock` locks one byte at this offset rather than the whole
/// file, so a reader in another process still reads the record while the
/// daemon holds the lock. Measured on this machine: with a whole-file
/// exclusive lock held by a second process, `ReadFile` fails with
/// `ERROR_LOCK_VIOLATION` (33); with the lock moved to this offset the same
/// call returns the record, and a second process taking the same offset is
/// still refused.
pub const RECORD_CAPACITY: u64 = 4096;

/// How often the daemon touches the record to prove it is alive.
///
/// The interval follows the precedent this workspace already has (Paseo's
/// daemon beats its own pid lock every 30 s). The multiple below does not,
/// because that daemon never reads its heartbeat back.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Beats a record may miss before it stops being believed.
///
/// A beat is one `SetFileTime` call: four missed in a row are not a slow
/// scheduler, they are a process that is not running. Three would leave a
/// single stalled beat one step from a false verdict. The cost of the fourth
/// is that a crash is reported up to two minutes late; the cost of erring the
/// other way is an app that believes a live daemon is gone.
pub const STALE_BEATS: u32 = 4;

/// The age past which a record is [`DaemonState::Stale`].
pub const STALE_AFTER: Duration =
    Duration::from_secs(HEARTBEAT_INTERVAL.as_secs() * STALE_BEATS as u64);

/// How long a goodbye keeps deciding.
///
/// A goodbye is dated by the same modification time a beat is, and it
/// outlives the process that wrote it — but not the record's authority: past
/// this window the reason is history, not an instruction. Keeping it deciding
/// would let a body nobody has touched in minutes disarm the crash brake
/// forever. The window is the heartbeat window: one age rule for the file, and
/// [`GOODBYE_CLOCK_SLACK`] is the only other side of it.
pub const GOODBYE_TRUSTED_FOR: Duration = STALE_AFTER;

/// How far ahead of this clock a goodbye may be dated and still be believed.
///
/// The date is written on this machine, on this clock, so the only thing a
/// future date can be is a correction that moved the clock between the write
/// and the read: a few seconds is as far as that goes, and a date further
/// ahead is one this clock cannot vouch for. Without the bound `heartbeat_age`
/// reads a future date as the freshest possible goodbye — `duration_since`
/// fails and the failure is read as zero — so the record would decide at any
/// distance, and start deciding again every time now caught up with it.
pub const GOODBYE_CLOCK_SLACK: Duration = Duration::from_secs(5);

/// Why a daemon stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitReason {
    /// Nobody was using it and the idle grace ran out.
    Idle,
    /// Someone asked: the `Shutdown` RPC.
    Requested,
    /// A reason this build cannot name: one a later version wrote, or one
    /// written here by a shutdown path that forgot to set its own.
    Unknown,
}

impl ExitReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ExitReason::Idle => "idle",
            ExitReason::Requested => "requested",
            ExitReason::Unknown => "unknown",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "idle" => ExitReason::Idle,
            "requested" => ExitReason::Requested,
            _ => ExitReason::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonRecord {
    pub pid: u32,
    pub instance_id: String,
    pub pipe_name: String,
    /// The listener is bound: a client connecting now will be admitted.
    pub ready: bool,
    /// Set once, on the way out, so the record says why it stopped.
    pub exit: Option<ExitReason>,
}

impl DaemonRecord {
    /// The record a daemon writes once it owns the lock, before it can serve.
    pub fn starting(pid: u32, instance_id: &str, pipe_name: &str) -> Self {
        Self {
            pid,
            instance_id: instance_id.to_string(),
            pipe_name: pipe_name.to_string(),
            ready: false,
            exit: None,
        }
    }

    /// The listener is bound; a probe that trusts this record may connect now.
    pub fn listening(&mut self) {
        self.ready = true;
    }

    /// The daemon is leaving, and this is why. Written before the lock is
    /// released, so a reader can never see a stopped record still holding the
    /// file it is about.
    pub fn stopped(&mut self, reason: ExitReason) {
        self.ready = false;
        self.exit = Some(reason);
    }

    pub fn body(&self) -> String {
        let mut body = format!(
            "pid={}\ninstance={}\npipe={}\n",
            self.pid, self.instance_id, self.pipe_name
        );
        if self.ready {
            body.push_str("ready=1\n");
        }
        if let Some(reason) = self.exit {
            body.push_str("exit=");
            body.push_str(reason.as_str());
            body.push('\n');
        }
        body
    }

    /// `None` for a body that is not a record — the pid and the instance are
    /// the pair that identifies a daemon, so a body missing either is not one.
    pub fn parse(body: &str) -> Option<Self> {
        let mut pid = None;
        let mut instance_id = None;
        let mut pipe_name = None;
        let mut ready = false;
        let mut exit = None;
        for line in body.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "pid" => pid = value.parse().ok(),
                "instance" => instance_id = Some(value.to_string()),
                "pipe" => pipe_name = Some(value.to_string()),
                "ready" => ready = value == "1",
                "exit" => exit = Some(ExitReason::parse(value)),
                _ => {}
            }
        }
        Some(Self {
            pid: pid?,
            instance_id: instance_id?,
            pipe_name: pipe_name.unwrap_or_default(),
            ready,
            exit,
        })
    }
}

/// What the record on disk says about the daemon it describes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DaemonState {
    /// Nothing to believe: no file, or a body that is not a record. A daemon
    /// that never ran and a file caught mid-rewrite read the same way, and
    /// neither is evidence of a crash.
    Absent,
    /// A record whose heartbeat is inside the window.
    Live(DaemonRecord),
    /// A record whose heartbeat has aged out: the daemon that wrote it died
    /// without a goodbye, or said goodbye long enough ago that the reason no
    /// longer decides anything. The body is still there to be read.
    Stale(DaemonRecord),
    /// A record that says why it stopped, inside [`GOODBYE_TRUSTED_FOR`]. The
    /// reason outlives the process; this state does not outlive the window.
    Stopped(DaemonRecord, ExitReason),
}

impl DaemonState {
    /// Read the record without connecting. This is the only question about a
    /// daemon that can be asked from outside without changing the answer: the
    /// accept path counts every connection and re-arms the idle timer.
    pub fn read(path: &Path) -> Self {
        let Ok(metadata) = std::fs::metadata(path) else {
            return Self::Absent;
        };
        let Ok(body) = std::fs::read_to_string(path) else {
            return Self::Absent;
        };
        let Some(record) = DaemonRecord::parse(&body) else {
            return Self::Absent;
        };
        match record.exit {
            Some(reason) if goodbye_decides(&metadata) => Self::Stopped(record, reason),
            Some(_) => Self::Stale(record),
            None if heartbeat_age(&metadata) <= STALE_AFTER => Self::Live(record),
            None => Self::Stale(record),
        }
    }

    pub fn record(&self) -> Option<&DaemonRecord> {
        match self {
            Self::Absent => None,
            Self::Live(record) | Self::Stale(record) | Self::Stopped(record, _) => Some(record),
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live(_))
    }

    /// True only for a record that is live *and* past its listener bind: the
    /// condition a readiness probe needs, as opposed to a liveness one.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Live(record) if record.ready)
    }
}

/// Whether a goodbye is dated close enough to now to still decide.
///
/// Only the goodbye gets the two-sided window. A future date on a *beat* means
/// a live daemon and is benign; a goodbye dated ahead is not an instruction
/// this clock can date, and `heartbeat_age`'s saturation to zero would
/// otherwise make it the freshest goodbye there is for as long as it sits
/// ahead of now.
fn goodbye_decides(metadata: &std::fs::Metadata) -> bool {
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    let now = SystemTime::now();
    match now.duration_since(modified) {
        Ok(age) => age <= GOODBYE_TRUSTED_FOR,
        Err(_) => modified
            .duration_since(now)
            .is_ok_and(|ahead| ahead <= GOODBYE_CLOCK_SLACK),
    }
}

/// The age a beat is judged by. A modification time that cannot be read, or
/// one ahead of now, reads as zero — "just beat" — which is the benign
/// direction for liveness and the reason a goodbye is dated by
/// [`goodbye_decides`] instead.
pub(crate) fn heartbeat_age(metadata: &std::fs::Metadata) -> Duration {
    metadata
        .modified()
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .unwrap_or_default()
}

/// The slice a beat's sleep is cut into, so a shutdown never waits a beat out.
const HEARTBEAT_SLICE: Duration = Duration::from_millis(50);

/// Keeps the record's modification time inside the staleness window.
///
/// A beat is the file's metadata, not its body: rewriting the body every
/// interval would put a torn read in front of every probe, and the body has
/// nothing to say that often.
pub struct Heartbeat {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Heartbeat {
    pub fn start(lock_file: &Path) -> io::Result<Self> {
        Self::with_interval(lock_file, HEARTBEAT_INTERVAL)
    }

    /// `write(true)` and no `create`: the file this beats on must be the one
    /// the lock owns, and a heartbeat that could create a lock file would
    /// publish a daemon that is not there.
    pub fn with_interval(lock_file: &Path, interval: Duration) -> io::Result<Self> {
        let file = OpenOptions::new().write(true).open(lock_file)?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("daemon-heartbeat".into())
                .spawn(move || {
                    while sleep_interruptibly(&stop, interval) {
                        if file.set_modified(SystemTime::now()).is_err() {
                            return;
                        }
                    }
                })?
        };
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }

    /// Idempotent: the shutdown path seals the record after this returns, and
    /// a beat still in flight would date the goodbye to the wrong instant.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Sleep the interval unless the stop flag is raised. `false` means it was.
fn sleep_interruptibly(stop: &AtomicBool, total: Duration) -> bool {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        thread::sleep(HEARTBEAT_SLICE.min(deadline - Instant::now()));
    }
    !stop.load(Ordering::SeqCst)
}

#[cfg(test)]
#[path = "daemon_record_tests.rs"]
mod tests;
