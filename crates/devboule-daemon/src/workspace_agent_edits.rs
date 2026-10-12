//! The files one workspace's agents changed, for the Changes panel of a folder
//! that is not a git repository.
//!
//! Orchestration and folding only: [`crate::agent_edit_records`] reads the
//! per-call records, and this module sums them per path. The sums are of what
//! each call recorded, never a net figure the daemon guessed.

use std::collections::BTreeMap;

use devboule_protocol::{
    AgentCallRef, DaemonMessage, WorkspaceAgentEditFile, WorkspaceAgentEdits, WorkspaceGitTotals,
};

use crate::agent_edit_records::{self, AgentEditRecord};
use crate::ServerState;

/// Most files one reply carries; past it the list is cut and `truncated`
/// says so.
pub(crate) const MAX_FILES: usize = 200;

/// Resolve `workspace_id` through the registry, never a request field, so an
/// unknown workspace is refused the way every other workspace read refuses it.
pub(crate) fn reply(state: &ServerState, id: u64, workspace_id: &str) -> DaemonMessage {
    let edits = match state.sessions.workspace_cwd(workspace_id) {
        Ok(_) => read(state, workspace_id),
        Err(error) => unavailable(error.message),
    };
    DaemonMessage::WorkspaceAgentEdits { id, edits }
}

fn read(state: &ServerState, workspace_id: &str) -> WorkspaceAgentEdits {
    let Some(journal) = state.sessions.journal_handle() else {
        return unavailable("the conversation journal is unavailable");
    };
    let read = agent_edit_records::read(journal, workspace_id, crate::journal::now_ms());
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

/// One row per path, sorted by path; the totals cover the rows kept.
fn fold(records: &[AgentEditRecord]) -> (Vec<WorkspaceAgentEditFile>, WorkspaceGitTotals, bool) {
    let mut by_path: BTreeMap<String, WorkspaceAgentEditFile> = BTreeMap::new();
    for record in records {
        let file = by_path
            .entry(record.path.clone())
            .or_insert_with(|| WorkspaceAgentEditFile {
                path: record.path.clone(),
                additions: 0,
                deletions: 0,
                last_agent: None,
                last_at_ms: 0,
                calls: Vec::new(),
            });
        file.additions += record.added;
        file.deletions += record.removed;
        // The records arrive oldest first, so the last one to pass here is
        // the newest edit on the file.
        if record.at_ms >= file.last_at_ms {
            file.last_at_ms = record.at_ms;
            file.last_agent = Some(record.agent.clone());
        }
        let call = AgentCallRef {
            session_id: record.session_id.clone(),
            tool_call_id: record.tool_call_id.clone(),
        };
        if !file.calls.contains(&call) {
            file.calls.push(call);
        }
    }
    let mut files: Vec<WorkspaceAgentEditFile> = by_path.into_values().collect();
    let capped = files.len() > MAX_FILES;
    files.truncate(MAX_FILES);
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
