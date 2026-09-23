//! The entries of one workspace folder, for the Files panel's tree.
//!
//! Orchestration only: resolve the folder from a workspace id — never a
//! request field — confine the requested path to it with the same two-layer
//! rule the diff uses (lexical components plus a walk that refuses links and
//! junctions, both through [`crate::workspace_git_support`], so the sentences
//! are shared and not copied), and hand the vouched directory to [`read`] for
//! the entries themselves. One directory per request: no recursion, no tree,
//! and nothing in this module writes.

use std::path::{Component, Path};

use devboule_protocol::{DaemonMessage, WorkspaceDirectory};

use crate::workspace_git_support::{
    confined, walk, Walked, NOT_A_DIRECTORY, OUTSIDE_THE_WORKSPACE,
};
use crate::ServerState;

#[path = "workspace_files_read.rs"]
mod read;

/// A path that is not a folder below an existing root: a file, or something
/// the walk statted that the panel may still name but never list.
const NOT_A_FOLDER: &str = "the requested path is not a folder";
/// A path whose component never had a stat: nothing exists there to list.
/// `pub(crate)`: the file read shows this same sentence for the same fact.
pub(crate) const DOES_NOT_EXIST: &str = "the requested path does not exist";
/// `.git` is excluded from every listing; naming it directly is the same
/// exclusion, not a second way in (DECISIONS §6: the tree lists everything
/// *but* this, and git stays the authority for everything inside it).
/// `pub(crate)`: the file read refuses the same spellings with this same
/// sentence — two copies of one refusal would be two truths.
pub(crate) const NOT_PART_OF_THE_TREE: &str =
    "the repository's own metadata folder is not part of the tree";

/// Resolve `workspace_id` through the registry — never a request field.
pub(crate) fn reply(state: &ServerState, id: u64, workspace_id: &str, path: &str) -> DaemonMessage {
    let directory = match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => directory_of(&root, path),
        // The sentence comes from the registry and names no path (see
        // `workspace_cwd`): it is safe to echo on this frame.
        Err(error) => refused(path, error.message),
    };
    DaemonMessage::WorkspaceFiles { id, directory }
}

/// The entries of one directory of one workspace. Private on purpose:
/// callers outside this module arrive through [`reply`], and the test
/// modules are children.
fn directory_of(root: &Path, requested: &str) -> WorkspaceDirectory {
    if requested.is_empty() {
        // The folder itself: there is no spelling to confine, but it must
        // exist as a directory — a workspace whose folder vanished after the
        // registry had cached it gets the same sentence the probe gives.
        if !root.is_dir() {
            return refused(requested, NOT_A_DIRECTORY);
        }
        return listed(root, requested);
    }
    let Some(target) = confined(root, requested) else {
        return refused(requested, OUTSIDE_THE_WORKSPACE);
    };
    if names_git_metadata(requested) {
        return refused(requested, NOT_PART_OF_THE_TREE);
    }
    match walk(root, requested) {
        Walked::Link(sentence) => refused(requested, sentence),
        Walked::Missing => refused(requested, DOES_NOT_EXIST),
        Walked::Inside(metadata) if !metadata.is_dir() => refused(requested, NOT_A_FOLDER),
        Walked::Inside(_) => listed(&target, requested),
    }
}

/// Whether any component of the requested spelling is the repository's own
/// metadata folder — in any spelling the filesystem resolves to it
/// ([`read::is_git_metadata`]: NTFS folds case, Win32 drops trailing
/// dots and spaces). Checked after confinement (so the components are
/// provably inside the root) and before the walk (so no stat of the metadata
/// folder happens at all). `pub(crate)`: `workspace_file_read` runs the very
/// same check — one function for both panels.
pub(crate) fn names_git_metadata(requested: &str) -> bool {
    Path::new(requested)
        .components()
        .any(|component| match component {
            Component::Normal(name) => read::is_git_metadata(&name.to_string_lossy()),
            _ => false,
        })
}

/// The entries of a directory already vouched for. `prefix` is the wire
/// spelling used to build each entry's own path; `requested` travels back
/// verbatim in `path`, like the diff's echo.
fn listed(directory: &Path, requested: &str) -> WorkspaceDirectory {
    let prefix = wire_prefix(requested);
    match read::read(directory, &prefix) {
        Ok(listed) => WorkspaceDirectory {
            path: requested.to_string(),
            entries: listed.entries,
            capped: listed.capped,
            skipped: listed.skipped,
            error: None,
        },
        Err(sentence) => refused(requested, sentence),
    }
}

/// The spelling entry paths are built from: the request's own components,
/// rejoined with `/` — so `src/`, `./src` and `src` all list the same
/// folder under the same child paths, and a dirty spelling can never make
/// `src//child` the key of an entry `src/child` already names.
fn wire_prefix(requested: &str) -> String {
    Path::new(requested)
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// A refusal to answer: `error` says what stopped it, and carries no
/// entries and no skips — the panel may then claim nothing about the folder
/// behind it (a `skipped` of a folder never listed would be a claim).
fn refused(requested: &str, sentence: impl Into<String>) -> WorkspaceDirectory {
    WorkspaceDirectory {
        path: requested.to_string(),
        entries: Vec::new(),
        capped: false,
        skipped: 0,
        error: Some(sentence.into()),
    }
}

#[cfg(test)]
#[path = "workspace_files_fixture.rs"]
mod fixture;
/// The rules of one listing — which entries survive, in which order, and how
/// many one reply carries — on plain folders with no repository in them.
/// Split by subject, not by line count: the refusals and the orchestration
/// are in [`tests`].
#[cfg(test)]
#[path = "workspace_files_read_tests.rs"]
mod read_tests;
#[cfg(test)]
#[path = "workspace_files_tests.rs"]
mod tests;
