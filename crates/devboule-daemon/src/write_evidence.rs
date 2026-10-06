//! Which session wrote which path of which repository, recorded only where a
//! tool KNOWS it wrote one. The daemon's such tool is the ACP host's
//! `fs/write_text_file` ([`crate::acp_host`]): it performs the write itself,
//! so the repository and the path are facts it read off its own `write` call,
//! not guesses made afterwards.
//!
//! A row is keyed by the repository's **root**, canonicalised, and by a path
//! relative to that root — never by the session's working directory. A
//! session's cwd may be a subdirectory of its workspace (that is the provider
//! client's choice, not the daemon's), and a key that depended on it would
//! make `<root>/sub/a.rs` and `<root>/a.rs` one key: a write in one file would
//! then be reported as a collision in another.
//!
//! Nothing here infers a write from a command line, a tool-call title or a
//! transcript. An agent that ran `sed -i`, an editor someone had open, and a
//! shell a person typed into are all invisible to this log — on purpose: the
//! transcript records what an agent *said* it was about to do, and a collision
//! report built from that would name a writer who may never have written. An
//! empty writer list therefore means "no session wrote this path through
//! Devboule", never "nobody touched this file".
//!
//! Bounded three times: rows older than the longest lookback a caller may ask
//! for are dropped on every append, the log itself is capped, and the instant
//! of the last eviction is kept so an answer can say it may be missing rows.
//! It is in memory and lives as long as this daemon run: the question it
//! answers is "who touched this in the last hour", and after a restart the
//! honest answer is empty rather than stale.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::journal::now_ms;

/// How a write was observed. The vocabulary is closed because only a tool
/// that performed the write may add to it: a second kind of evidence is a
/// different mechanism, not a differently-worded claim about this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteEvidence {
    /// The ACP host wrote these bytes itself, for `fs/write_text_file`.
    AgentFileWrite,
}

impl WriteEvidence {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AgentFileWrite => "agent_file_write",
        }
    }

    /// The confidence this evidence carries. Nothing here is inferred from a
    /// transcript or a command line, so there is no lower grade to report.
    pub(crate) fn confidence(self) -> &'static str {
        match self {
            Self::AgentFileWrite => "high",
        }
    }
}

/// One performed write: the session that asked for it, the repository and
/// path it resolved to, the instant it happened and what kind of write it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PathWrite {
    pub session_id: String,
    /// The repository root's canonical path, as [`repo_key`] spells it.
    pub repo: String,
    /// Relative to that root.
    pub path: String,
    pub at_ms: u64,
    pub evidence: WriteEvidence,
}

/// The writers of one path, and whether that list can be read as whole.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WriterList {
    pub writers: Vec<PathWrite>,
    /// Set when rows were dropped inside the window this list was read over:
    /// the log was full, so a write older than the eviction may be missing and
    /// an absent writer is not proof that nobody wrote the file.
    pub may_be_incomplete: bool,
}

/// Rows kept before the oldest is dropped. Each row is one file write by one
/// session, so this is a generous ceiling for a day of history; past it the
/// log answers for the recent past and says nothing about the rest.
pub(crate) const MAX_ROWS: usize = 512;

/// The longest lookback any caller may ask the log for, and therefore the
/// horizon its rows are kept across. The tool refuses a larger window at its
/// own parser; this is what makes that refusal true rather than advisory.
pub(crate) const MAX_LOOKBACK: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Default)]
struct Log {
    rows: Vec<PathWrite>,
    /// When each repository last lost a row for room, and nothing else: an
    /// overflow in one repository says nothing about the next one's list, and
    /// an eviction older than the horizon is not worth remembering.
    evicted_at_ms: HashMap<String, u64>,
}

/// One bounded log of path writes. Production drives the process-wide
/// [`log`]; a test that needs rows of its own builds an instance rather than
/// filling the shared one, so what one test writes cannot decide what another
/// reads.
#[derive(Default)]
pub(crate) struct WriteLog {
    inner: Mutex<Log>,
}

static LOG: OnceLock<WriteLog> = OnceLock::new();

/// This daemon's one writer log, where every performed write is recorded.
pub(crate) fn log() -> &'static WriteLog {
    LOG.get_or_init(WriteLog::default)
}

/// The repository `root` belongs to, as the canonical path both ends of a
/// match compare. `None` when the folder is not in a repository, or has gone:
/// a caller that cannot name its own repository has no writer list to read.
pub(crate) fn repo_key(root: &Path) -> Option<String> {
    std::fs::canonicalize(root)
        .ok()
        .map(|canonical| canonical.to_string_lossy().into_owned())
}

impl WriteLog {
    /// Record that `session_id` wrote `written`, an absolute path a tool has just
    /// written, resolving the repository from the session's own `cwd`. A write
    /// outside every repository is dropped: no collision sweep can ever ask about
    /// it, and a row nobody can match only costs memory.
    pub(crate) fn record_path_write(&self, session_id: &str, cwd: &Path, written: &Path) {
        if session_id.is_empty() {
            return;
        }
        let Some(root) =
            repository_root_above(cwd).and_then(|root| std::fs::canonicalize(root).ok())
        else {
            return;
        };
        let Ok(written) = std::fs::canonicalize(written) else {
            return;
        };
        let Ok(relative) = written.strip_prefix(&root) else {
            return;
        };
        let Some(path) = spelling(&relative.to_string_lossy()) else {
            return;
        };
        let now = now_ms();
        let mut log = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let horizon = horizon_ms(now);
        log.rows.retain(|row| row.at_ms >= horizon);
        log.evicted_at_ms.retain(|_, at| *at >= horizon);
        log.rows.push(PathWrite {
            session_id: session_id.to_string(),
            repo: root.to_string_lossy().into_owned(),
            path,
            at_ms: now,
            evidence: WriteEvidence::AgentFileWrite,
        });
        if log.rows.len() > MAX_ROWS {
            let excess = log.rows.len() - MAX_ROWS;
            let dropped: Vec<String> = log.rows.drain(..excess).map(|row| row.repo).collect();
            for repo in dropped {
                log.evicted_at_ms.insert(repo, now);
            }
        }
    }

    /// The newest row per session for `subject` inside `repo`, newest first, and
    /// whether the log dropped rows inside that window. One row per session, not
    /// per write: a caller wants to know who is in the file, and a session that
    /// wrote it forty times is one collision, not forty.
    pub(crate) fn writers_for(&self, repo: &str, subject: &str, lookback: Duration) -> WriterList {
        let Some(wanted) = spelling(subject) else {
            return WriterList::default();
        };
        let since = now_ms().saturating_sub(lookback.as_millis() as u64);
        let log = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let mut writers: Vec<PathWrite> = Vec::new();
        for row in log.rows.iter().rev() {
            if row.at_ms < since || row.repo != repo || row.path != wanted {
                continue;
            }
            if !writers.iter().any(|kept| kept.session_id == row.session_id) {
                writers.push(row.clone());
            }
        }
        WriterList {
            writers,
            // Conservative on purpose: this repository's own eviction inside the
            // window may have dropped a row the caller would otherwise have been
            // shown.
            may_be_incomplete: log.evicted_at_ms.get(repo).is_some_and(|at| *at >= since),
        }
    }
}

/// The repository root at or above `folder`: the nearest ancestor holding a
/// `.git` entry, which is a folder in a plain checkout and a file in a linked
/// worktree. Git's own discovery walks the same way.
fn repository_root_above(folder: &Path) -> Option<PathBuf> {
    folder
        .ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The one spelling two ends of a match agree on: `\` folded to `/`, and case
/// folded on the platforms whose filesystems fold it — NTFS and the default
/// APFS volume. A path spelled `Src/A.rs` there is the file the walk found as
/// `src/a.rs`, and a caller naming it either way asks about the same file.
pub(crate) fn spelling(path: &str) -> Option<String> {
    let folded = path.replace('\\', "/");
    let trimmed = folded.trim();
    if trimmed.is_empty() {
        return None;
    }
    #[cfg(any(windows, target_os = "macos"))]
    let spelling = trimmed.to_lowercase();
    #[cfg(not(any(windows, target_os = "macos")))]
    let spelling = trimmed.to_string();
    Some(spelling)
}

fn horizon_ms(now: u64) -> u64 {
    now.saturating_sub(MAX_LOOKBACK.as_millis() as u64)
}
