//! The wire shapes of the agent-edit reads: which files one workspace's agents
//! changed, and the diff of one of those files.
//!
//! The rows come from the journal, not from the disk, so a file an agent
//! changed and then deleted still has its record, and a folder that is not a
//! git repository still has its history.

use serde::{Deserialize, Serialize};

use crate::messages::{WorkspaceGitDiffLine, WorkspaceGitTotals};

/// One tool call of one agent. A reference, not a copy: the call's text is read
/// back through [`crate::ClientMessage::WorkspaceAgentDiff`].
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCallRef {
    pub session_id: String,
    pub tool_call_id: String,
}

/// One file the workspace's agents changed, with every call that changed it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAgentEditFile {
    /// Relative to the workspace with forward slashes, or the provider's own
    /// spelling for a file outside it.
    pub path: String,
    /// The sum of the lines each recorded call added and removed. Not a net
    /// figure: two calls that add and then remove a line count both.
    pub additions: u64,
    pub deletions: u64,
    /// The name of the agent behind the file's newest recorded edit.
    pub last_agent: Option<String>,
    /// When that newest edit was recorded, in epoch milliseconds.
    pub last_at_ms: u64,
    pub calls: Vec<AgentCallRef>,
}

/// The files this workspace's agents changed, newest edits within the last
/// week, one row per path, sorted by path.
///
/// `totals` covers the rows this reply carries, so the header and the list
/// agree. `truncated` says rows were dropped by a bound — the file cap, the
/// session cap, or a transcript too large to replay — and `error` is a refusal
/// to answer at all.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAgentEdits {
    pub files: Vec<WorkspaceAgentEditFile>,
    pub totals: WorkspaceGitTotals,
    pub truncated: bool,
    pub error: Option<String>,
}

/// Whether a [`WorkspaceAgentFileDiff`] carries every line its edits recorded.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceAgentDiffStatus {
    Ok,
    /// Some recorded text was cut to its budget, so the lines are a prefix of
    /// what the agent sent.
    Truncated,
    /// No lines came back; `error` says why.
    Error,
}

/// The diff of one file as the agents made it: each selected call's rows in
/// order, with a header naming the edit when a file has more than one.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAgentFileDiff {
    /// The path this reply is about, echoed as the caller sent it.
    pub path: String,
    pub lines: Vec<WorkspaceGitDiffLine>,
    pub status: WorkspaceAgentDiffStatus,
    /// Why no lines came back, in one static sentence with no absolute path.
    /// `null` exactly when `status` is not `error`.
    pub error: Option<String>,
}

#[cfg(test)]
#[path = "agent_edit_reads_tests.rs"]
mod tests;
