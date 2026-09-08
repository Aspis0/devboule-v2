use serde::{Deserialize, Serialize};

/// A folder registered by the user. Git metadata is intentionally kept out of
/// this first wire shape; it is persisted by the daemon for later project
/// metadata surfaces.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// The checkout used as the working directory of a session.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceIsolation {
    Local,
    Worktree,
}

/// Public workspace metadata. The path is kept in the journal record and is
/// deliberately not duplicated on this wire type; session creation resolves
/// it from `id` in the daemon.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub isolation: WorkspaceIsolation,
}
