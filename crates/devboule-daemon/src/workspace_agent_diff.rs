//! One path's agent-edit diff, built from the records the journal holds.
//!
//! A request that names a call is answered from that call's own edits; one
//! that names only a path is answered from every edit recorded for it, in
//! time order. A replace pair is compared line by line
//! ([`crate::line_diff::replace_lines`]); a provider's own unified diff is read
//! as it was sent ([`crate::line_diff::unified_lines`]). Line numbers are not
//! carried: a record has no position in the file, and a number this module
//! could not prove is worse than none.

use devboule_protocol::{
    AgentCallRef, AgentEditText, DaemonMessage, WorkspaceAgentDiffStatus, WorkspaceAgentFileDiff,
    WorkspaceGitDiffLine, WorkspaceGitDiffLineKind,
};

use crate::agent_edit_records::{self, AgentEditRecord};
use crate::line_diff;
use crate::ServerState;

/// Resolve `workspace_id` through the registry, then read the path's records.
/// The read is workspace-scoped, so a `call` naming another workspace's
/// session has no record here and is refused.
pub(crate) fn reply(
    state: &ServerState,
    id: u64,
    workspace_id: &str,
    path: &str,
    call: Option<AgentCallRef>,
) -> DaemonMessage {
    let diff = match state.sessions.workspace_cwd(workspace_id) {
        Ok(_) => read(state, workspace_id, path, call.as_ref()),
        Err(error) => refused(path, error.message),
    };
    DaemonMessage::WorkspaceAgentDiff { id, diff }
}

fn read(
    state: &ServerState,
    workspace_id: &str,
    path: &str,
    call: Option<&AgentCallRef>,
) -> WorkspaceAgentFileDiff {
    let Some(journal) = state.sessions.journal_handle() else {
        return refused(path, "the conversation journal is unavailable");
    };
    let read = agent_edit_records::read(journal, workspace_id, crate::journal::now_ms());
    if let Some(error) = read.error {
        return refused(path, error);
    }
    let selected: Vec<&AgentEditRecord> = read
        .records
        .iter()
        .filter(|record| record.path == path)
        .filter(|record| match call {
            Some(call) => {
                record.session_id == call.session_id && record.tool_call_id == call.tool_call_id
            }
            None => true,
        })
        .collect();
    if selected.is_empty() {
        return refused(
            path,
            if call.is_some() {
                "no agent edit is recorded for this call in this workspace"
            } else {
                "no agent edit is recorded for this path"
            },
        );
    }
    let (lines, truncated) = lines_for(&selected);
    WorkspaceAgentFileDiff {
        path: path.to_string(),
        lines,
        status: if truncated {
            WorkspaceAgentDiffStatus::Truncated
        } else {
            WorkspaceAgentDiffStatus::Ok
        },
        error: None,
    }
}

/// The rows of every selected edit, in order. An edit with rows is labelled
/// `Edit n of m` when more than one of them has any; an edit with no text
/// contributes nothing.
fn lines_for(records: &[&AgentEditRecord]) -> (Vec<WorkspaceGitDiffLine>, bool) {
    let built: Vec<(Vec<WorkspaceGitDiffLine>, bool)> = records
        .iter()
        .map(|record| (record_lines(record), record.truncated))
        .collect();
    let contributing = built.iter().filter(|(rows, _)| !rows.is_empty()).count();
    let mut lines = Vec::new();
    let mut truncated = false;
    let mut seen = 0usize;
    for (rows, cut) in built {
        truncated |= cut;
        if rows.is_empty() {
            continue;
        }
        seen += 1;
        if contributing > 1 {
            lines.push(WorkspaceGitDiffLine {
                kind: WorkspaceGitDiffLineKind::Header,
                text: format!("Edit {seen} of {contributing}"),
            });
        }
        lines.extend(rows);
    }
    (lines, truncated)
}

fn record_lines(record: &AgentEditRecord) -> Vec<WorkspaceGitDiffLine> {
    match &record.text {
        Some(AgentEditText::Replace { old, new }) => line_diff::replace_lines(old, new),
        Some(AgentEditText::Diff { text }) => line_diff::unified_lines(text),
        None => Vec::new(),
    }
}

fn refused(path: &str, message: impl Into<String>) -> WorkspaceAgentFileDiff {
    WorkspaceAgentFileDiff {
        path: path.to_string(),
        lines: Vec::new(),
        status: WorkspaceAgentDiffStatus::Error,
        error: Some(message.into()),
    }
}

#[cfg(test)]
#[path = "workspace_agent_diff_tests.rs"]
mod tests;
