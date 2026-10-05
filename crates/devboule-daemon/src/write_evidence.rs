//! Which session wrote which path, recorded only where a tool KNOWS it wrote
//! one. The daemon's such tool is the ACP host's `fs/write_text_file`
//! ([`crate::acp_host`]): it performs the write itself, so the path is a fact
//! it read off its own `write` call, not a guess made afterwards.
//!
//! Nothing here infers a write from a command line, a tool-call title or a
//! transcript. An agent that ran `sed -i`, an editor someone had open, and a
//! shell a person typed into are all invisible to this log — on purpose: the
//! transcript records what an agent *said* it was about to do, and a collision
//! report built from that would name a writer who may never have written. An
//! empty writer list therefore means "no session wrote this path through
//! Devboule", never "nobody touched this file".
//!
//! Bounded twice: rows older than the longest lookback a caller may ask for
//! are dropped on every append, and the log itself is capped, so a session
//! that writes a file per turn cannot grow it without limit. It is in memory
//! and lives as long as this daemon run: the question it answers is "who
//! touched this in the last hour", and after a restart the honest answer is
//! empty rather than stale.

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

/// One performed write: the session that asked for it, the path it resolved
/// to, the instant it happened and what kind of write it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PathWrite {
    pub session_id: String,
    /// Workspace-relative, in the separator spelling both ends normalize to.
    pub path: String,
    pub at_ms: u64,
    pub evidence: WriteEvidence,
}

/// Rows kept before the oldest is dropped. Each row is one file write by one
/// session, so this is a generous ceiling for a day of history; past it the
/// log answers for the recent past and says nothing about the rest.
const MAX_ROWS: usize = 512;

/// The longest lookback any caller may ask the log for, and therefore the
/// horizon its rows are kept across. The tool refuses a larger window at its
/// own parser; this is what makes that refusal true rather than advisory.
pub(crate) const MAX_LOOKBACK: Duration = Duration::from_secs(24 * 60 * 60);

static LOG: OnceLock<Mutex<Vec<PathWrite>>> = OnceLock::new();

fn rows() -> &'static Mutex<Vec<PathWrite>> {
    LOG.get_or_init(|| Mutex::new(Vec::new()))
}

/// Record that `session_id` wrote `path`, in the spelling
/// [`spelling`] gives it. Only a tool that has just written the file may call
/// this; an empty path is dropped rather than stored, because a row nobody can
/// match again is a row that only costs memory.
pub(crate) fn record_path_write(session_id: &str, path: &str) {
    let path = match spelling(path) {
        Some(path) => path,
        None => return,
    };
    if session_id.is_empty() {
        return;
    }
    let now = now_ms();
    let mut rows = rows().lock().unwrap_or_else(|error| error.into_inner());
    let horizon_ms = horizon_ms(now);
    rows.retain(|row| row.at_ms >= horizon_ms);
    rows.push(PathWrite {
        session_id: session_id.to_string(),
        path,
        at_ms: now,
        evidence: WriteEvidence::AgentFileWrite,
    });
    if rows.len() > MAX_ROWS {
        let excess = rows.len() - MAX_ROWS;
        rows.drain(..excess);
    }
}

/// The newest row per session for `path` inside `lookback`, newest first.
/// One row per session, not per write: a caller wants to know who is in the
/// file, and a session that wrote it forty times is one collision, not forty.
pub(crate) fn writers_for(path: &str, lookback: Duration) -> Vec<PathWrite> {
    let Some(wanted) = spelling(path) else {
        return Vec::new();
    };
    let since = now_ms().saturating_sub(lookback.as_millis() as u64);
    let mut found: Vec<PathWrite> = Vec::new();
    for row in rows()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .rev()
    {
        if row.at_ms < since || row.path != wanted {
            continue;
        }
        if !found.iter().any(|kept| kept.session_id == row.session_id) {
            found.push(row.clone());
        }
    }
    found
}

/// The one spelling two ends of a match agree on: `\` folded to `/`, and on
/// Windows case folded too, because that is the filesystem's own rule there
/// and a path spelled `Src\A.rs` is the file the walk found as `src\a.rs`.
pub(crate) fn spelling(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return None;
    }
    #[cfg(windows)]
    let normalized = trimmed.to_ascii_lowercase();
    #[cfg(not(windows))]
    let normalized = trimmed.to_string();
    Some(normalized)
}

fn horizon_ms(now: u64) -> u64 {
    now.saturating_sub(MAX_LOOKBACK.as_millis() as u64)
}
