use serde::{Deserialize, Serialize};

/// Whether a project folder can host a worktree.
///
/// The daemon records what its own git probe said about the folder when it was
/// registered (`ProjectRecord.git_state`), and this is that word read for the
/// New workspace sheet. `Unknown` is every word that claims nothing — a missing
/// git program, a timed-out probe, a daemon that predates the field — so a
/// reader keeps the option rather than hiding a folder's way to a second
/// checkout on a failed probe.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeSupport {
    Supported,
    Unsupported,
    #[default]
    Unknown,
}

/// A folder registered by the user: its identity, and what the daemon's probe
/// found for it at registration time.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    /// A hint read off the recorded git state, not a live fact: the daemon
    /// re-checks the folder before a worktree is added. An older daemon sends
    /// no field at all, which reads as [`WorktreeSupport::Unknown`].
    #[serde(default)]
    pub worktree_support: WorktreeSupport,
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
