//! Where an edited file sits in its workspace: the path a stored edit names,
//! resolved against the directory its provider ran in and confined to the
//! workspace root. A path that leaves the root has no place in the workspace's
//! records, so the reads never serve it.

use std::path::{Component, Path, PathBuf};

use devboule_protocol::{AgentToolEdit, SessionEvent};

/// One step of a lexically normalised path.
#[derive(Debug, PartialEq, Eq)]
enum Part {
    /// A drive prefix or the root separator: the path's anchor.
    Anchor(String),
    Name(String),
}

/// The path of `stored` relative to the workspace `root`, `/`-separated, or
/// `None` when it is not inside `root`. A relative `stored` was written against
/// the provider's own directory, `session_cwd`, so it is joined to that first.
pub(crate) fn workspace_relative(
    stored: &str,
    session_cwd: Option<&Path>,
    root: &Path,
) -> Option<String> {
    let given = Path::new(stored);
    let resolved = if given.is_absolute() {
        parts(given)?
    } else {
        parts(&session_cwd.unwrap_or(root).join(given))?
    };
    let root_parts = parts(root)?;
    let rest = strip_root(&resolved, &root_parts)?;
    (!rest.is_empty()).then(|| rest.join("/"))
}

/// The form two spellings of one file share: lowercase on Windows, where
/// `Src\Lib.rs` and `src/lib.rs` name the same file, as spelled otherwise.
pub(crate) fn path_key(path: &str) -> String {
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.to_string()
    }
}

/// The components of `path` with `.` dropped and `..` applied. `None` when a
/// `..` climbs above the path's anchor.
fn parts(path: &Path) -> Option<Vec<Part>> {
    let mut out: Vec<Part> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                out.push(Part::Anchor(
                    prefix.as_os_str().to_string_lossy().into_owned(),
                ));
            }
            Component::RootDir => out.push(Part::Anchor("/".to_string())),
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Part::Name(_)) => {
                    out.pop();
                }
                _ => return None,
            },
            Component::Normal(name) => {
                out.push(Part::Name(name.to_string_lossy().into_owned()));
            }
        }
    }
    Some(out)
}

/// The names of `resolved` below `root`, or `None` when `root` is not a prefix
/// of it. Windows compares them ignoring case.
fn strip_root(resolved: &[Part], root: &[Part]) -> Option<Vec<String>> {
    if resolved.len() < root.len() {
        return None;
    }
    let (head, tail) = resolved.split_at(root.len());
    if !head
        .iter()
        .zip(root)
        .all(|(left, right)| same_part(left, right))
    {
        return None;
    }
    tail.iter()
        .map(|part| match part {
            Part::Name(name) => Some(name.clone()),
            Part::Anchor(_) => None,
        })
        .collect()
}

fn same_part(left: &Part, right: &Part) -> bool {
    let (left, right) = match (left, right) {
        (Part::Anchor(left), Part::Anchor(right)) | (Part::Name(left), Part::Name(right)) => {
            (left, right)
        }
        _ => return false,
    };
    if cfg!(windows) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

/// Where a session's stored edit paths resolve: the directory its provider ran
/// in, and the workspace root they must stay inside. No root drops every edit.
#[derive(Clone, Debug, Default)]
pub(crate) struct EditScope {
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) root: Option<PathBuf>,
}

/// Keeps a tool call's file changes only where they are inside the workspace
/// root, each rewritten root-relative. An edit outside the root, or any edit
/// when no root is known, leaves the event: a subscriber or a replay never
/// receives text for a file outside the workspace.
pub(crate) fn confine_event(event: &mut SessionEvent, scope: &EditScope) {
    let (SessionEvent::AgentToolCall { edits, .. } | SessionEvent::AgentToolUpdate { edits, .. }) =
        event
    else {
        return;
    };
    let Some(list) = edits.take() else {
        return;
    };
    let kept: Vec<AgentToolEdit> = match scope.root.as_deref() {
        None => Vec::new(),
        Some(root) => list
            .into_iter()
            .filter_map(|mut edit| {
                edit.path = workspace_relative(&edit.path, scope.cwd.as_deref(), root)?;
                Some(edit)
            })
            .collect(),
    };
    *edits = (!kept.is_empty()).then_some(kept);
}

#[cfg(test)]
#[path = "agent_edit_path_tests.rs"]
mod tests;
