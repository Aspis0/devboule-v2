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
use crate::ci_watch_quota::{make_room, Refusal};

const CI_WATCH_FILE: &str = "ci_watches.json";
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// A delivered watch is history; it is kept this long, then forgotten.
const DELIVERED_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const MAX_WATCHES: usize = 500;

static WATCH_COUNTER: AtomicU64 = AtomicU64::new(1);

/// What a keyed insert did.
#[derive(Clone, Debug)]
pub(crate) enum Admit {
    /// The key already had a watch, which is the one this call answers.
    Existing(CiWatchRecord),
    /// The watch is new and stored.
    Inserted(CiWatchRecord),
}

/// Why a watch was not stored.
#[derive(Debug)]
pub(crate) enum InsertError {
    /// Its session or its repository already holds as many as it may.
    Quota(Refusal),
    /// The whole store is busy with unfinished watches.
    Full,
    Io(io::Error),
}

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
    /// Settled after an uncertain send: the bytes may already be out, so the
    /// wake is never made again. Reads as delivered, with the uncertainty
    /// kept in the value.
    DeliveredUncertain,
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
    /// The branch whose head this watch follows, in branch mode: the `sha`
    /// above is the head resolved when the watch was started, and a head that
    /// no longer matches it supersedes the watch.
    #[serde(default)]
    pub(crate) branch: Option<String>,
    pub(crate) created_at_ms: u64,
    pub(crate) state: CiState,
    /// The redacted, bounded verdict text, once the watch has finished.
    pub(crate) summary: Option<String>,
    /// `<watch id>:<state>`: the key a wake is made under.
    pub(crate) wake_key: Option<String>,
    pub(crate) wake: Wake,
    /// Whether a person approved the one infra retry when the watch was
    /// started. Without it there is never a retry.
    #[serde(default)]
    pub(crate) retry_approved: bool,
    /// 0 until the one approved retry has been spent; never reset.
    #[serde(default)]
    pub(crate) retry_count: u32,
    /// Whether `gh` accepted the re-run. A count of 1 with this false is a
    /// spend whose answer was never recorded — a daemon that died in that
    /// window — so nobody can say whether the re-run went out.
    #[serde(default)]
    pub(crate) retry_issued: bool,
    /// The workflow runs that retry asked for, so the record says which ids
    /// were asked for and not only how many times.
    #[serde(default)]
    pub(crate) retried_runs: Vec<u64>,
    /// The check run ids the retry was decided on: the old attempt's whole
    /// evidence, so the new attempt can be told from it and the old failed
    /// checks dropped instead of judged a second time.
    #[serde(default)]
    pub(crate) retry_evidence: Vec<u64>,
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

    /// Test-only: move a watch's start `by` milliseconds into the past, so a
    /// test can reach the overdue close without waiting for it.
    #[cfg(test)]
    pub(crate) fn age(&self, watch_id: &str, by_ms: u64) {
        let mut records = self.records();
        if let Some(record) = records
            .iter_mut()
            .find(|record| record.watch_id == watch_id)
        {
            record.created_at_ms = record.created_at_ms.saturating_sub(by_ms);
        }
        let _ = self.persist(&records);
    }

    /// Test-only: put a spent retry back to unconfirmed, which is what a
    /// daemon restart finds when it died between the reservation and the
    /// answer `gh` never got to record.
    #[cfg(test)]
    pub(crate) fn leave_retry_unissued(&self, watch_id: &str) {
        let mut records = self.records();
        if let Some(record) = records
            .iter_mut()
            .find(|record| record.watch_id == watch_id)
        {
            record.retry_issued = false;
        }
        let _ = self.persist(&records);
    }

    /// Test-only: put a settled wake back to claimed, which is what a daemon
    /// restart finds when it died between the claim and the send.
    #[cfg(test)]
    pub(crate) fn leave_claim_unsettled(&self, watch_id: &str) {
        let mut records = self.records();
        if let Some(record) = records
            .iter_mut()
            .find(|record| record.watch_id == watch_id)
        {
            record.wake = Wake::Sending;
        }
        let _ = self.persist(&records);
    }

    /// The watch this session already has on this commit, if any. Test-only:
    /// production admits through [`CiWatchStore::find_or_insert`], which looks
    /// the key up and stores under one lock. The key is the whole watch —
    /// session, repository, commit and the branch it follows — so a commit
    /// watched as a sha and the same commit watched as a branch head are two
    /// watches, and only the second one follows a head.
    #[cfg(test)]
    pub(crate) fn find(
        &self,
        session_id: &str,
        slug: &str,
        sha: &str,
        branch: Option<&str>,
    ) -> Option<CiWatchRecord> {
        self.records()
            .iter()
            .find(|record| {
                record.session_id == session_id
                    && record.sha == sha
                    && record.slug() == slug
                    && record.branch.as_deref() == branch
            })
            .cloned()
    }

    /// The watch this session already has on this key, or the one just
    /// stored: one look and one write under one lock, so two calls racing for
    /// the same key cannot both insert and end up with two retry counters on
    /// one commit.
    pub(crate) fn find_or_insert(&self, record: CiWatchRecord) -> Result<Admit, InsertError> {
        let mut records = self.records();
        if let Some(existing) = records
            .iter()
            .find(|existing| same_watch(existing, &record))
            .cloned()
        {
            return Ok(Admit::Existing(existing));
        }
        push_with_room(&mut records, record.clone())?;
        self.persist(&records).map_err(InsertError::Io)?;
        Ok(Admit::Inserted(record))
    }

    /// Test-only: seed a watch without asking the key, which is how a test
    /// fills a store to its bounds.
    #[cfg(test)]
    pub(crate) fn insert(&self, record: CiWatchRecord) -> Result<(), InsertError> {
        let mut records = self.records();
        push_with_room(&mut records, record)?;
        self.persist(&records).map_err(InsertError::Io)
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

    /// Record the person's yes for the one infra retry on a watch that is
    /// still open: a later call may bring an approval the first one did not
    /// have, and it is the same watch. `Ok(None)` means the watch is terminal
    /// or gone, so the yes changes nothing.
    pub(crate) fn approve_retry(&self, watch_id: &str) -> io::Result<Option<CiWatchRecord>> {
        let mut records = self.records();
        let Some(record) = records
            .iter_mut()
            .find(|record| record.watch_id == watch_id && !record.state.is_terminal())
        else {
            return Ok(None);
        };
        if record.retry_approved {
            return Ok(Some(record.clone()));
        }
        record.retry_approved = true;
        let approved = record.clone();
        if let Err(error) = self.persist(&records) {
            if let Some(record) = records
                .iter_mut()
                .find(|record| record.watch_id == watch_id)
            {
                record.retry_approved = false;
            }
            return Err(error);
        }
        Ok(Some(approved))
    }

    /// Reserve the watch's one approved retry on `runs`, before any re-run is
    /// asked for: a daemon that dies after this finds the retry reserved and
    /// never issues it twice. `false` means another pass already reserved it,
    /// or the record could not be written — either way nothing may be issued.
    pub(crate) fn note_retry_reserved(
        &self,
        watch_id: &str,
        runs: &[u64],
        evidence: &[u64],
    ) -> bool {
        let mut records = self.records();
        let Some(record) = records
            .iter_mut()
            .find(|record| record.watch_id == watch_id && !record.state.is_terminal())
        else {
            return false;
        };
        if record.retry_count > 0 {
            return false;
        }
        record.retry_count = 1;
        record.retried_runs = runs.to_vec();
        record.retry_evidence = evidence.to_vec();
        if self.persist(&records).is_err() {
            if let Some(record) = records
                .iter_mut()
                .find(|record| record.watch_id == watch_id)
            {
                record.retry_count = 0;
                record.retried_runs.clear();
                record.retry_evidence.clear();
            }
            // The spend is what keeps the retry at one, so an unrecorded
            // spend issues nothing and the watch tries again next pass.
            eprintln!("ci watch: could not reserve the retry of {watch_id}; it is not issued");
            return false;
        }
        true
    }

    /// Record that `gh` took the re-run: the difference between a retry that
    /// was only asked for and one that is really on its way.
    pub(crate) fn mark_retry_issued(&self, watch_id: &str) -> io::Result<()> {
        let mut records = self.records();
        let Some(record) = records
            .iter_mut()
            .find(|record| record.watch_id == watch_id && !record.state.is_terminal())
        else {
            return Ok(());
        };
        record.retry_issued = true;
        self.persist(&records)
    }

    /// Settle a claim whose send may already be out: never given back, never
    /// made again. At-most-once beats at-least-once for a verdict wake.
    pub(crate) fn settle_wake_uncertain(&self, watch_id: &str) -> io::Result<()> {
        let mut records = self.records();
        let Some(record) = records
            .iter_mut()
            .find(|record| record.watch_id == watch_id && record.wake == Wake::Sending)
        else {
            return Ok(());
        };
        record.wake = Wake::DeliveredUncertain;
        self.persist(&records)
    }
}

/// The one key a watch is unique by: the session that asked, the repository,
/// the commit, and whether it follows a branch — and which one.
fn same_watch(left: &CiWatchRecord, right: &CiWatchRecord) -> bool {
    left.session_id == right.session_id
        && left.sha == right.sha
        && left.branch == right.branch
        && left.host.eq_ignore_ascii_case(&right.host)
        && left.repo_owner.eq_ignore_ascii_case(&right.repo_owner)
        && left.repo.eq_ignore_ascii_case(&right.repo)
}

/// Fit one more watch in, ageing out finished history first: the bounds are
/// the caller's to refuse, so this only reports them.
fn push_with_room(
    records: &mut Vec<CiWatchRecord>,
    record: CiWatchRecord,
) -> Result<(), InsertError> {
    make_room(records, &record).map_err(InsertError::Quota)?;
    let now = now_ms();
    // Terminal watches age out: finished history is kept a bounded time
    // whatever its wake did, so dead sessions cannot fill the store.
    records.retain(|existing| {
        let terminal = existing.state.is_terminal();
        let fresh = now.saturating_sub(existing.created_at_ms) < DELIVERED_RETENTION_MS;
        if terminal && !fresh {
            if matches!(existing.wake, Wake::Pending | Wake::Sending) {
                eprintln!(
                    "ci watch: dropping undelivered wake {} (owner session ended)",
                    existing.watch_id
                );
            }
            return false;
        }
        true
    });
    // Still full means genuinely busy, or a burst of recent history: make
    // room from the oldest finished watch rather than refuse the tool
    // forever. Only a store with nothing finished still refuses, and that
    // refusal clears as watches finish.
    while records.len() >= MAX_WATCHES {
        let oldest = records
            .iter()
            .enumerate()
            .filter(|(_, existing)| existing.state.is_terminal())
            .min_by_key(|(_, existing)| existing.created_at_ms)
            .map(|(index, _)| index);
        let Some(index) = oldest else {
            return Err(InsertError::Full);
        };
        let dropped = records.remove(index);
        if matches!(dropped.wake, Wake::Pending | Wake::Sending) {
            eprintln!(
                "ci watch: evicting undelivered wake {} (owner session ended)",
                dropped.watch_id
            );
        }
    }
    records.push(record);
    Ok(())
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
