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

/// Public workspace metadata.
///
/// `path` is the checkout in display form. It used to be omitted because
/// every workspace WAS the project folder and session creation could resolve
/// it from `id`. A worktree checkout is new information a user cannot derive
/// from the project — the third time in this protocol a "deliberately
/// omitted" wire field became load-bearing. Check the premise before
/// dropping a field like this again.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub isolation: WorkspaceIsolation,
    /// Checkout directory. Display form: no Windows verbatim `\\?\` prefix.
    pub path: String,
}
