//! The record the Oracle endpoint in the app publishes for the daemon: the
//! port to reach and the bearer to reach it with.
//!
//! Same rules as the daemon's own record (`daemon_record.rs`): a `key=value`
//! body, liveness published as the file's modification time and judged from
//! that mtime alone. The pid written here is diagnostic only — a file cannot
//! prove the process behind it is the same one, and a recycled pid is the
//! failure this format exists to stop repeating. The body is written once,
//! after the listener binds (`ready=1`), and the file is deleted on the way
//! out; a crash leaves it behind and the age decides.

use std::path::{Path, PathBuf};

use crate::daemon_record::{heartbeat_age, STALE_AFTER};
use crate::paths::RuntimePaths;

/// One file, like the daemon: the record this carries is written inside the
/// lock that owns it, and the name says lock.
pub const ORACLE_APP_LOCK_FILE_NAME: &str = "oracle-app.lock";

pub fn oracle_app_lock_path(paths: &RuntimePaths) -> PathBuf {
    paths.dir.join(ORACLE_APP_LOCK_FILE_NAME)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OracleAppRecord {
    pub pid: u32,
    pub instance: String,
    pub port: u16,
    pub token: String,
    /// The listener is bound: a reader that trusts this record may connect.
    pub ready: bool,
}

impl OracleAppRecord {
    /// The record before the bind. Like the daemon's `starting`, it exists so
    /// `ready` has an off state to move out of; the app only ever writes the
    /// body once [`Self::listening`] has been called.
    pub fn new(pid: u32, instance: &str, port: u16, token: &str) -> Self {
        Self {
            pid,
            instance: instance.to_string(),
            port,
            token: token.to_string(),
            ready: false,
        }
    }

    /// The listener is bound; a probe that trusts this record may connect now.
    pub fn listening(&mut self) {
        self.ready = true;
    }

    pub fn body(&self) -> String {
        let mut body = format!(
            "pid={}\ninstance={}\nport={}\ntoken={}\n",
            self.pid, self.instance, self.port, self.token
        );
        if self.ready {
            body.push_str("ready=1\n");
        }
        body
    }

    /// `None` for a body that is not a record: pid, instance, port and token
    /// are what a caller needs to reach the app, so a body missing any of
    /// them is not one.
    pub fn parse(body: &str) -> Option<Self> {
        let mut pid = None;
        let mut instance = None;
        let mut port = None;
        let mut token = None;
        let mut ready = false;
        for line in body.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "pid" => pid = value.parse().ok(),
                "instance" => instance = Some(value.to_string()),
                "port" => port = value.parse().ok(),
                "token" => token = Some(value.to_string()),
                "ready" => ready = value == "1",
                _ => {}
            }
        }
        Some(Self {
            pid: pid?,
            instance: instance?,
            port: port?,
            token: token?,
            ready,
        })
    }
}

/// What the record on disk says about the endpoint it describes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OracleAppState {
    /// No file, or a body that is not a record: nothing to believe, and a
    /// file caught mid-rewrite reads the same way.
    Absent,
    /// A record whose heartbeat is inside the window.
    Live(OracleAppRecord),
    /// A record whose heartbeat aged out: the app died without saying
    /// goodbye. The body is still there to be read.
    Stale(OracleAppRecord),
}

impl OracleAppState {
    /// One side-effect-free read — the daemon asks this per call, the same
    /// way the app reads the daemon's record.
    pub fn read(path: &Path) -> Self {
        let Ok(metadata) = std::fs::metadata(path) else {
            return Self::Absent;
        };
        let Ok(body) = std::fs::read_to_string(path) else {
            return Self::Absent;
        };
        let Some(record) = OracleAppRecord::parse(&body) else {
            return Self::Absent;
        };
        if heartbeat_age(&metadata) <= STALE_AFTER {
            Self::Live(record)
        } else {
            Self::Stale(record)
        }
    }

    pub fn record(&self) -> Option<&OracleAppRecord> {
        match self {
            Self::Absent => None,
            Self::Live(record) | Self::Stale(record) => Some(record),
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

#[cfg(test)]
#[path = "oracle_app_record_tests.rs"]
mod tests;
