//! The uncommitted working-tree state of one workspace, for the Changes panel.
//!
//! Orchestration only: resolve the root, run `git`, hand the bytes to
//! [`parse`] and compose the reply. The root comes from a workspace id, never
//! from the caller's `path` field (the wire declares that display-only).
//! Untracked files are in neither `git diff --numstat` dump, so their lines
//! are counted by [`parse`] under a byte cap that flags itself on the row
//! instead of passing for an exact count.

use std::collections::HashMap;
use std::path::Path;

use devboule_protocol::{DaemonMessage, WorkspaceGitRow, WorkspaceGitStatus, WorkspaceGitTotals};

use crate::git::{
    detect_git_repository, run_git_args, GitOutput, GitRepositoryStatus, GitRunError,
    GIT_STDOUT_MAX_BYTES,
};
use crate::ServerState;

#[path = "workspace_git_status_parse.rs"]
mod parse;

/// A workspace folder below a repository root. Read from a subdirectory,
/// `git status` answers for the whole repository with paths relative to that
/// subdirectory, so the panel would show files of other checkouts and count
/// every untracked one against a path that is not there. Decided in the fix
/// round: refuse and say what the panel would have shown instead of resolving
/// the top level, which is a product choice this slice does not make.
const INSIDE_A_REPOSITORY: &str =
    "this workspace folder is inside a git repository but is not its root; the Changes panel \
     lists changes of the repository, not of this folder";

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
    if !root.is_dir() {
        // The path is deliberately absent: `error` crosses a wire whose
        // redaction seam does not touch this frame (see `WorkspaceGitStatus`).
        // This one sentence also covers the folder that disappeared after the
        // registry had cached it.
        return unavailable("the workspace folder is not a directory");
    }
    match detect_git_repository(root) {
        GitRepositoryStatus::RepositoryRoot => {}
        GitRepositoryStatus::InsideRepository => return caveat(INSIDE_A_REPOSITORY),
        GitRepositoryStatus::NotRepository => return build(false, false, None, Vec::new(), None),
        GitRepositoryStatus::TimedOut => {
            return unavailable("git did not answer within the probe timeout")
        }
        GitRepositoryStatus::Unknown => return unavailable("git could not be run"),
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

fn git(root: &Path, subcommand: &[&str]) -> Result<GitOutput, GitRunError> {
    let mut arguments = vec!["-C".to_string(), root.to_string_lossy().into_owned()];
    arguments.extend(subcommand.iter().map(|argument| (*argument).to_string()));
    run_git_args(&arguments)
}

fn run_error(error: GitRunError, operation: &str) -> String {
    match error {
        GitRunError::NotFound => format!("{operation}: git is not installed"),
        GitRunError::TimedOut => format!("{operation}: git timed out"),
        GitRunError::SpawnFailed => format!("{operation}: git could not be started"),
    }
}

/// A failed command's identity and exit code, and never its stderr: git
/// writes absolute paths and personal file names into stderr, and `error`
/// travels on a wire whose redaction seam does not touch this frame. The
/// detail is dropped on purpose, not lost by accident — a local debug session
/// that needs it should print `output.stderr` at the call site.
fn exit_error(operation: &str, output: &GitOutput) -> String {
    match output.code {
        Some(code) => format!("{operation} exited with code {code}"),
        None => format!("{operation} was terminated before it could report a code"),
    }
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
