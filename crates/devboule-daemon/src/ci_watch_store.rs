//! The persistent CI watches: what the daemon is waiting on and what it still
//! owes the session that asked.
//!
//! One phrase: remember each watch across a restart, and make a verdict wake
//! its owner at most once. The completion is written with its idempotency key
//! (`<watch id>:<state>`) before any wake is tried, a wake is claimed on disk
//! before the message is sent, and only a claim that provably did not send is
//! given back — so a restart or a retry can find a wake to make, never a wake
//! made twice.
//!
//! The file sits beside the journal and is replaced whole through the
//! owner-only writer; one that cannot be read is moved aside, never trusted.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::ci_summary::CiState;

const CI_WATCH_FILE: &str = "ci_watches.json";
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// A delivered watch is history; it is kept this long, then forgotten.
const DELIVERED_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const MAX_WATCHES: usize = 500;

static WATCH_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Where the wake of a finished watch stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Wake {
    /// The watch is still running; nothing is owed.
    NotDue,
    /// Finished and recorded; the message has not been sent.
    Pending,
    /// Claimed and being sent. A restart that finds this does not send again:
    /// it cannot know whether the message got out.
    Sending,
    Delivered,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CiWatchRecord {
    pub(crate) watch_id: String,
    pub(crate) session_id: String,
    pub(crate) owner_user: String,
    pub(crate) owner_client: String,
    pub(crate) host: String,
    pub(crate) repo_owner: String,
    pub(crate) repo: String,
    pub(crate) sha: String,
    pub(crate) created_at_ms: u64,
    pub(crate) state: CiState,
    /// The redacted, bounded verdict text, once the watch has finished.
    pub(crate) summary: Option<String>,
    /// `<watch id>:<state>`: the key a wake is made under.
    pub(crate) wake_key: Option<String>,
    pub(crate) wake: Wake,
}

impl CiWatchRecord {
    pub(crate) fn slug(&self) -> String {
        format!("{}/{}", self.repo_owner, self.repo)
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// A fresh watch id: unique across restarts of one daemon.
pub(crate) fn new_watch_id() -> String {
    format!(
        "ciw.{:x}.{:x}",
        now_ms(),
        WATCH_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) struct CiWatchStore {
    path: PathBuf,
    records: Mutex<Vec<CiWatchRecord>>,
}

impl CiWatchStore {
    pub(crate) fn load(runtime_dir: &Path) -> Self {
        let path = runtime_dir.join(CI_WATCH_FILE);
        let records = match read_records(&path) {
            Ok(records) => records,
            Err(reason) => {
                eprintln!(
                    "ci watch: {} is unusable ({reason}); starting with no watches",
                    path.display()
                );
                let _ = std::fs::rename(&path, path.with_extension("corrupt"));
                Vec::new()
            }
        };
        Self {
            path,
            records: Mutex::new(records),
        }
    }

    fn records(&self) -> std::sync::MutexGuard<'_, Vec<CiWatchRecord>> {
        self.records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Replace the file with `records`. Callers hold the lock, so two writes
    /// cannot interleave their read-and-replace.
    fn persist(&self, records: &[CiWatchRecord]) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(records).map_err(io::Error::other)?;
        crate::atomic::write_protected_bytes(&self.path, &bytes)
    }

    #[cfg(test)]
    pub(crate) fn get(&self, watch_id: &str) -> Option<CiWatchRecord> {
        self.records()
            .iter()
            .find(|record| record.watch_id == watch_id)
            .cloned()
    }

    /// The watch this session already has on this commit, if any: asking twice
    /// answers the first watch instead of starting a second one.
    pub(crate) fn find(&self, session_id: &str, slug: &str, sha: &str) -> Option<CiWatchRecord> {
        self.records()
            .iter()
            .find(|record| {
                record.session_id == session_id && record.sha == sha && record.slug() == slug
            })
            .cloned()
    }

    pub(crate) fn insert(&self, record: CiWatchRecord) -> io::Result<()> {
        let mut records = self.records();
        let now = now_ms();
        records.retain(|existing| {
            existing.wake != Wake::Delivered
                || now.saturating_sub(existing.created_at_ms) < DELIVERED_RETENTION_MS
        });
        if records.len() >= MAX_WATCHES {
            return Err(io::Error::other("too many CI watches are being kept"));
        }
        records.push(record);
        self.persist(&records)
    }

    /// Watches still waiting on CI.
    pub(crate) fn open(&self) -> Vec<CiWatchRecord> {
        self.records()
            .iter()
            .filter(|record| !record.state.is_terminal())
            .cloned()
            .collect()
    }

    /// Finished watches whose wake has not been made.
    pub(crate) fn pending_wakes(&self) -> Vec<CiWatchRecord> {
        self.records()
            .iter()
            .filter(|record| record.wake == Wake::Pending)
            .cloned()
            .collect()
    }

    pub(crate) fn set_state(&self, watch_id: &str, state: CiState) -> io::Result<()> {
        let mut records = self.records();
        let Some(record) = records
            .iter_mut()
            .find(|record| record.watch_id == watch_id && !record.state.is_terminal())
        else {
            return Ok(());
        };
        if record.state == state {
            return Ok(());
        }
        record.state = state;
        self.persist(&records)
    }

    /// Record the verdict and the wake it owes, once. A watch that already
    /// finished keeps its first verdict.
    pub(crate) fn complete(
        &self,
        watch_id: &str,
        state: CiState,
        summary: String,
    ) -> io::Result<bool> {
        let mut records = self.records();
        let Some(record) = records
            .iter_mut()
            .find(|record| record.watch_id == watch_id && !record.state.is_terminal())
        else {
            return Ok(false);
        };
        record.state = state;
        record.summary = Some(summary);
        record.wake_key = Some(format!("{watch_id}:{}", state.as_str()));
        record.wake = Wake::Pending;
        self.persist(&records)?;
        Ok(true)
    }

    /// Take the right to make a wake. Only the one caller that moves
    /// `Pending` to `Sending` gets the record; the move is on disk before it
    /// returns.
    pub(crate) fn claim_wake(&self, watch_id: &str) -> Option<CiWatchRecord> {
        let mut records = self.records();
        let record = records
            .iter_mut()
            .find(|record| record.watch_id == watch_id && record.wake == Wake::Pending)?;
        record.wake = Wake::Sending;
        let claimed = record.clone();
        if self.persist(&records).is_err() {
            if let Some(record) = records
                .iter_mut()
                .find(|record| record.watch_id == watch_id)
            {
                record.wake = Wake::Pending;
            }
            return None;
        }
        Some(claimed)
    }

    /// Settle a claim: delivered, or given back because nothing was sent.
    pub(crate) fn finish_wake(&self, watch_id: &str, delivered: bool) -> io::Result<()> {
        let mut records = self.records();
        let Some(record) = records
            .iter_mut()
            .find(|record| record.watch_id == watch_id && record.wake == Wake::Sending)
        else {
            return Ok(());
        };
        record.wake = if delivered {
            Wake::Delivered
        } else {
            Wake::Pending
        };
        self.persist(&records)
    }
}

fn read_records(path: &Path) -> Result<Vec<CiWatchRecord>, String> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.len() > MAX_FILE_BYTES {
        return Err("larger than the store allows".to_string());
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "ci_watch_store_tests.rs"]
mod tests;
