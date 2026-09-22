//! The uncommitted working-tree state of one workspace, for the Changes panel.
//!
//! Orchestration only: resolve the root, classify it and run `git` through
//! [`crate::workspace_git_support`], hand the bytes to [`parse`] and compose
//! the reply. The root comes from a workspace id, never from the caller's
//! `path` field (the wire declares that display-only). Untracked files are
//! in neither `git diff --numstat` dump, so their lines are counted by
//! [`parse`] under a byte cap that flags itself on the row instead of
//! passing for an exact count.

use std::collections::HashMap;
use std::path::Path;

use devboule_protocol::{DaemonMessage, WorkspaceGitRow, WorkspaceGitStatus, WorkspaceGitTotals};

use crate::git::GIT_STDOUT_MAX_BYTES;
use crate::workspace_git_support::{exit_error, git, probe, run_error, Probe, INSIDE_A_REPOSITORY};
use crate::ServerState;

#[path = "workspace_git_status_parse.rs"]
mod parse;

/// Resolve `workspace_id` through the registry — never a request field.
pub(crate) fn reply(state: &ServerState, id: u64, workspace_id: &str) -> DaemonMessage {
    let status = match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => status_of(&root),
        Err(error) => unavailable(error.message),
    };
    DaemonMessage::WorkspaceGit { id, status }
}

/// The status of one directory. Private on purpose: callers outside this
/// module arrive through [`reply`], and the test modules are children.
fn status_of(root: &Path) -> WorkspaceGitStatus {
    match probe(root) {
        Probe::Ready => {}
        Probe::NotRepository => return build(false, false, None, Vec::new(), None),
        Probe::InsideRepository => return caveat(INSIDE_A_REPOSITORY),
        // Not a directory (also: the folder vanished after the registry had
        // cached it), a probe git did not answer, or git missing.
        Probe::Refused(message) => return unavailable(message),
    }
    let arguments = [
        "status",
        "--porcelain=v2",
        "--branch",
        "--untracked-files=all",
        "-z",
    ];
    let output = match git(root, &arguments) {
        Ok(output) => output,
        Err(error) => return caveat(run_error(error, "git status")),
    };
    if !output.success {
        return caveat(exit_error("git status", &output));
    }
    let (branch, entries) = parse::parse_status(&output.stdout);
    // The accumulator stops at `GIT_STDOUT_MAX_BYTES + 1`, so anything longer
    // was cut. A list the daemon knows to be partial is withheld rather than
    // handed over as the whole tree; the header records arrive first, so the
    // branch and `dirty` still stand.
    if parse::stdout_truncated(&output.stdout) {
        return build(
            true,
            true,
            branch,
            Vec::new(),
            Some(format!(
                "git status produced more than the {GIT_STDOUT_MAX_BYTES}-byte reply cap; the \
                 row list is withheld rather than cut short"
            )),
        );
    }
    let (counts, count_error) = numstat(root);
    let degraded = count_error.is_some();
    let rows: Vec<WorkspaceGitRow> = entries
        .into_iter()
        .map(|(path, status)| parse::build_row(root, &path, status, &counts, degraded))
        .collect();
    // Normally `!rows.is_empty()`. The withheld list above is the one
    // deliberate exception and never reaches this line.
    let dirty = !rows.is_empty();
    build(true, dirty, branch, rows, count_error)
}

/// Assemble the reply; `totals` is summed from the rows it is given.
fn build(
    is_git: bool,
    dirty: bool,
    branch: Option<String>,
    rows: Vec<WorkspaceGitRow>,
    error: Option<String>,
) -> WorkspaceGitStatus {
    let mut totals = WorkspaceGitTotals::default();
    for row in &rows {
        totals.additions += row.additions;
        totals.deletions += row.deletions;
    }
    WorkspaceGitStatus {
        is_git,
        dirty,
        branch,
        totals,
        rows,
        error,
    }
}

/// No answer at all: `is_git` false and `error` saying why.
fn unavailable(message: impl Into<String>) -> WorkspaceGitStatus {
    build(false, false, None, Vec::new(), Some(message.into()))
}

/// A repository the probe already proved, whose contents this reply cannot
/// give: `is_git` stands, `error` names what is missing.
fn caveat(message: impl Into<String>) -> WorkspaceGitStatus {
    build(true, false, None, Vec::new(), Some(message.into()))
}

/// Line counts per path over both halves of the index: `git diff` reads the
/// worktree against the index, `git diff --cached` the index against its
/// tree. Two calls rather than one against `HEAD`, which a repository with no
/// commit yet does not have. The first failure is reported, and the rows come
/// back regardless — `error` then distrusts their numbers, not the list.
fn numstat(root: &Path) -> (HashMap<String, parse::Counted>, Option<String>) {
    let mut counts = HashMap::new();
    let mut error: Option<String> = None;
    let calls: [&[&str]; 2] = [
        &["diff", "--numstat", "-z"],
        &["diff", "--cached", "--numstat", "-z"],
    ];
    for subcommand in calls {
        let failure = match git(root, subcommand) {
            Ok(output) if output.success => {
                parse::merge_numstat(&output.stdout, &mut counts);
                // Same guard as the status branch: the accumulator cuts at
                // `GIT_STDOUT_MAX_BYTES` and exits 0, so only the length can
                // say it happened. Unreachable while `status` runs first —
                // every numstat record is shorter than the status record of
                // the same path (measured ~126 vs ~240 bytes at a 120-byte
                // path) — and kept because the cap is shared, not per call.
                parse::stdout_truncated(&output.stdout).then(|| {
                    format!(
                        "git diff produced more than the {GIT_STDOUT_MAX_BYTES}-byte reply cap; \
                         the line counts are a floor, not a count"
                    )
                })
            }
            Ok(output) => Some(exit_error("git diff", &output)),
            Err(failure) => Some(run_error(failure, "git diff")),
        };
        if error.is_none() {
            error = failure;
        }
    }
    (counts, error)
}

#[cfg(test)]
#[path = "workspace_git_status_tests.rs"]
mod tests;
