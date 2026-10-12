//! The files one workspace's agents changed, for the Changes panel of a folder
//! that is not a git repository.
//!
//! Orchestration and folding only: [`crate::agent_edit_records`] reads the
//! per-call records, and this module sums them per path. The sums are of what
//! each call recorded, never a net figure the daemon guessed.

use std::collections::BTreeMap;
use std::path::Path;

use devboule_protocol::{
    AgentCallRef, DaemonMessage, WorkspaceAgentEditFile, WorkspaceAgentEdits, WorkspaceGitTotals,
};

use crate::agent_edit_path::path_key;
use crate::agent_edit_records::{self, AgentEditRecord};
use crate::ServerState;

/// Most files one reply carries; past it the list is cut and `truncated`
/// says so.
pub(crate) const MAX_FILES: usize = 200;

/// Most calls one file row names. A file edited by more calls keeps these and
/// its row says it was cut.
pub(crate) const MAX_CALLS_PER_FILE: usize = 200;

/// The encoded bytes the rows of one reply may take: well under the 1 MiB frame
/// cap, so a busy folder cannot make the reply unencodable.
pub(crate) const MAX_REPLY_BYTES: usize = 512 * 1024;

/// Resolve `workspace_id` through the registry, never a request field, so an
/// unknown workspace is refused the way every other workspace read refuses it.
pub(crate) fn reply(state: &ServerState, id: u64, workspace_id: &str) -> DaemonMessage {
    let edits = match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => read(state, workspace_id, &root),
        Err(error) => unavailable(error.message),
    };
    DaemonMessage::WorkspaceAgentEdits { id, edits }
}

fn read(state: &ServerState, workspace_id: &str, root: &Path) -> WorkspaceAgentEdits {
    let Some(journal) = state.sessions.journal_handle() else {
        return unavailable("the conversation journal is unavailable");
    };
    let read = agent_edit_records::read(journal, workspace_id, root, crate::journal::now_ms());
    if let Some(error) = read.error {
        return unavailable(error);
    }
    let (files, totals, capped) = fold(&read.records);
    WorkspaceAgentEdits {
        files,
        totals,
        truncated: read.truncated || capped,
        error: None,
    }
}

/// One row per file, sorted by path key; the totals cover the rows kept. Two
/// spellings of one file share a row and show the newest edit's spelling.
fn fold(records: &[AgentEditRecord]) -> (Vec<WorkspaceAgentEditFile>, WorkspaceGitTotals, bool) {
    let mut by_path: BTreeMap<String, WorkspaceAgentEditFile> = BTreeMap::new();
    for record in records {
        let file =
            by_path
                .entry(path_key(&record.path))
                .or_insert_with(|| WorkspaceAgentEditFile {
                    path: record.path.clone(),
                    additions: 0,
                    deletions: 0,
                    last_agent: None,
                    last_at_ms: 0,
                    calls: Vec::new(),
                    truncated: false,
                });
        file.additions += record.added;
        file.deletions += record.removed;
        file.truncated |= record.truncated;
        // The records arrive oldest first, so the last one to pass here is
        // the newest edit on the file.
        if record.at_ms >= file.last_at_ms {
            file.last_at_ms = record.at_ms;
            file.last_agent = Some(record.agent.clone());
            file.path = record.path.clone();
        }
        let call = AgentCallRef {
            session_id: record.session_id.clone(),
            tool_call_id: record.tool_call_id.clone(),
        };
        if file.calls.contains(&call) {
            continue;
        }
        if file.calls.len() < MAX_CALLS_PER_FILE {
            file.calls.push(call);
        } else {
            file.truncated = true;
        }
    }
    let mut files: Vec<WorkspaceAgentEditFile> = Vec::new();
    // The array's brackets; each row adds its own comma below.
    let mut reply_bytes = 2usize;
    let mut capped = false;
    for file in by_path.into_values() {
        let row_bytes = serde_json::to_vec(&file).map_or(usize::MAX, |bytes| bytes.len() + 1);
        if files.len() == MAX_FILES || reply_bytes.saturating_add(row_bytes) > MAX_REPLY_BYTES {
            capped = true;
            break;
        }
        reply_bytes += row_bytes;
        files.push(file);
    }
    let mut totals = WorkspaceGitTotals::default();
    for file in &files {
        totals.additions += file.additions;
        totals.deletions += file.deletions;
    }
    (files, totals, capped)
}

fn unavailable(message: impl Into<String>) -> WorkspaceAgentEdits {
    WorkspaceAgentEdits {
        files: Vec::new(),
        totals: WorkspaceGitTotals::default(),
        truncated: false,
        error: Some(message.into()),
    }
}

#[cfg(test)]
#[path = "workspace_agent_edits_tests.rs"]
mod tests;
