//! The uncommitted diff of one workspace file, for the Changes panel's
//! detail view. Orchestration only: confine the requested path to the
//! workspace folder, classify the folder and run `git` through
//! [`crate::workspace_git_support`], hand the bytes to [`parse`] and
//! compose the reply. The folder comes from a workspace id; the caller's
//! `path` is a relative path this module proves stays inside that folder
//! before anything is opened or spawned — the `assertWithinWorkspace` of
//! Paseo's file explorer, in the same role.

use std::path::Path;

use devboule_protocol::{
    DaemonMessage, WorkspaceGitDiffLine, WorkspaceGitDiffLineKind, WorkspaceGitDiffStatus,
    WorkspaceGitFileDiff,
};

use crate::git::{GitOutput, GIT_STDOUT_MAX_BYTES};
use crate::workspace_git_support::{
    confined, exit_error, git, probe, run_error, walk, Walked, OUTSIDE_THE_WORKSPACE,
};
use crate::ServerState;

#[path = "workspace_git_diff_parse.rs"]
mod parse;

/// Bytes of one file this reply will read or diff: Paseo's per-file cap
/// (`checkout-git.ts:2101`). Past it the file comes back `too_large` with no
/// lines — refused whole, never cut short. Checked with one `stat` before
/// any process is spawned; [`parse`] bounds its own read by the same
/// constant, for the file that grows between the two.
const DIFF_FILE_MAX_BYTES: u64 = 1024 * 1024;

/// Resolve `workspace_id` through the registry — never a request field.
pub(crate) fn reply(state: &ServerState, id: u64, workspace_id: &str, path: &str) -> DaemonMessage {
    let file = match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => diff_of(&root, path),
        // The sentence comes from the registry and names no path (see
        // `workspace_cwd`): it is safe to echo on this frame.
        Err(error) => refused(path, error.message),
    };
    DaemonMessage::WorkspaceGitFile { id, file }
}

/// The diff of one file of one directory. Private on purpose: callers
/// outside this module arrive through [`reply`], and the test modules are
/// children.
fn diff_of(root: &Path, requested: &str) -> WorkspaceGitFileDiff {
    // Confinement first and without a process: nothing below opens a path
    // this check has not put inside `root`.
    let Some(target) = confined(root, requested) else {
        return refused(requested, OUTSIDE_THE_WORKSPACE);
    };
    if let Some(sentence) = probe(root).refusal() {
        return refused(requested, sentence);
    }
    // Then the walk: the confinement above trusts the spelling, this trusts
    // the filesystem — every component stat'ed without following, a link
    // refused with the shared sentence, a component with no stat (a deletion)
    // ending the walk: nothing below it exists, and git answers for the
    // missing path.
    let final_metadata = match walk(root, requested) {
        Walked::Link(sentence) => return refused(requested, sentence),
        Walked::Missing => None,
        Walked::Inside(metadata) => Some(metadata),
    };
    // The final entry must be an ordinary file of this workspace: refuse a
    // directory (an ordinary one — a junction ended the walk above), and
    // apply the per-file cap in the same stat. A path with no stat has no
    // size and reaches git below, which is the only side that can tell a
    // deletion from a name git never knew.
    if let Some(metadata) = final_metadata {
        if metadata.is_dir() {
            return refused(requested, "the requested path is a folder, not a file");
        }
        if metadata.len() > DIFF_FILE_MAX_BYTES {
            return withheld(
                requested,
                format!(
                    "the file is larger than the {DIFF_FILE_MAX_BYTES}-byte per-file cap; its \
                     diff is not handed back"
                ),
            );
        }
    }
    // The pathspec is literal: a file named `a[1].txt` must not be read as
    // a glob that happens to match itself today.
    let pathspec = format!(":(literal){requested}");
    let status_output = match git(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--untracked-files=all",
            "-z",
            "--",
            &pathspec,
        ],
    ) {
        Ok(output) if output.success => output,
        Ok(output) => return refused(requested, exit_error("git status", &output)),
        Err(error) => return refused(requested, run_error(error, "git status")),
    };
    match parse::path_record(&status_output.stdout) {
        // Untracked: `git diff` has nothing to say about a path it does not
        // track, so [`parse`] synthesizes the lines the file itself carries.
        Some(parse::PathRecord::Untracked) => match parse::untracked_file(&target) {
            parse::UntrackedFile::Lines(lines) => composed(requested, true, false, lines),
            parse::UntrackedFile::Binary => binary(requested),
            parse::UntrackedFile::TooLarge => withheld(
                requested,
                format!(
                    "the file grew past the {DIFF_FILE_MAX_BYTES}-byte per-file cap while it was \
                     being read; its diff is not handed back"
                ),
            ),
            parse::UntrackedFile::Unreadable => refused(requested, "the file could not be opened"),
        },
        Some(parse::PathRecord::Tracked) => {
            let output = match diff_output(root, &pathspec) {
                Ok(output) => output,
                Err(sentence) => return refused(requested, sentence),
            };
            // The accumulator stopped at `GIT_STDOUT_MAX_BYTES + 1` raw
            // bytes while git exited 0: only the length says so. The key is
            // the lossy String's length, and it cannot miss a cut —
            // `from_utf8_lossy` only grows or keeps length (an invalid run
            // of at most 3 raw bytes becomes one 3-byte replacement),
            // measured over every 3-, 4- and 5-byte sequence of the
            // interesting alphabet (`logs/probes-fix-slice2/`): a cut at
            // cap+1 never comes back at or under the cap. The converse —
            // raw under the cap, string over it — is not a false positive:
            // the string is what would travel. A diff the daemon knows to be
            // cut is withheld whole — the lines of a hunk missing
            // their halves are worse than no lines.
            if output.stdout.len() > GIT_STDOUT_MAX_BYTES {
                return withheld(
                    requested,
                    format!(
                        "the diff is larger than the {GIT_STDOUT_MAX_BYTES}-byte reply cap; the \
                         lines are withheld rather than cut short"
                    ),
                );
            }
            let parsed = parse::parse_diff(&output.stdout);
            if parsed.binary {
                return binary(requested);
            }
            composed(requested, parsed.is_new, parsed.is_deleted, parsed.lines)
        }
        // No record: either a path git tracks — and with nothing to report,
        // which is git's own answer, including for a file `assume-unchanged`
        // hides from the status (measured) — or a path this repository does
        // not track at all. `git ls-files` decides, with the same literal
        // pathspec. `exists()` would not do: through a directory junction it
        // answers for a path outside this workspace (a one-bit oracle), and
        // an ignored file exists too without ever being a change of the
        // workspace — both are refused below for what is true of each: git
        // tracks no diff to give.
        None => match git(root, &["ls-files", "-z", "--", &pathspec]) {
            Ok(output) if output.success => {
                if output.stdout.is_empty() {
                    refused(
                        requested,
                        "git does not track the requested path so there is no diff to give",
                    )
                } else {
                    composed(requested, false, false, Vec::new())
                }
            }
            Ok(output) => refused(requested, exit_error("git ls-files", &output)),
            Err(error) => refused(requested, run_error(error, "git ls-files")),
        },
    }
}

/// `git diff HEAD` — staged and unstaged together, the delta from the last
/// commit — with the one declared fallback: a repository with no commit yet
/// has no `HEAD` (git exits 128, measured) and `git diff --cached` answers
/// for its index instead. Any other failure reports as itself.
fn diff_output(root: &Path, pathspec: &str) -> Result<GitOutput, String> {
    match git(root, &["diff", "HEAD", "--", pathspec]) {
        Ok(output) if output.success => Ok(output),
        Ok(output) if output.code == Some(128) => {
            match git(root, &["diff", "--cached", "--", pathspec]) {
                Ok(staged) if staged.success => Ok(staged),
                Ok(staged) => Err(exit_error("git diff", &staged)),
                Err(error) => Err(run_error(error, "git diff")),
            }
        }
        Ok(output) => Err(exit_error("git diff", &output)),
        Err(error) => Err(run_error(error, "git diff")),
    }
}

/// A reply with lines: the counts are the lines' own — a `header` is not a
/// changed line and is never counted.
fn composed(
    requested: &str,
    is_new: bool,
    is_deleted: bool,
    lines: Vec<WorkspaceGitDiffLine>,
) -> WorkspaceGitFileDiff {
    let additions = lines
        .iter()
        .filter(|line| line.kind == WorkspaceGitDiffLineKind::Add)
        .count() as u64;
    let deletions = lines
        .iter()
        .filter(|line| line.kind == WorkspaceGitDiffLineKind::Remove)
        .count() as u64;
    WorkspaceGitFileDiff {
        path: requested.to_string(),
        is_new,
        is_deleted,
        additions,
        deletions,
        lines,
        status: WorkspaceGitDiffStatus::Ok,
        error: None,
    }
}

/// A complete answer about a file whose lines are deliberately absent.
fn binary(requested: &str) -> WorkspaceGitFileDiff {
    without_lines(requested, WorkspaceGitDiffStatus::Binary, None)
}

/// Lines exist and were withheld: `error` names the cap that did it.
fn withheld(requested: &str, sentence: String) -> WorkspaceGitFileDiff {
    without_lines(requested, WorkspaceGitDiffStatus::TooLarge, Some(sentence))
}

/// A refusal to answer: `error` says what stopped it.
fn refused(requested: &str, sentence: impl Into<String>) -> WorkspaceGitFileDiff {
    without_lines(
        requested,
        WorkspaceGitDiffStatus::Error,
        Some(sentence.into()),
    )
}

fn without_lines(
    requested: &str,
    status: WorkspaceGitDiffStatus,
    error: Option<String>,
) -> WorkspaceGitFileDiff {
    WorkspaceGitFileDiff {
        path: requested.to_string(),
        is_new: false,
        is_deleted: false,
        additions: 0,
        deletions: 0,
        lines: Vec::new(),
        status,
        error,
    }
}

#[cfg(test)]
#[path = "workspace_git_diff_fixture.rs"]
mod fixture;
#[cfg(test)]
#[path = "workspace_git_diff_tests.rs"]
mod tests;
/// The replies that carry no lines — `binary`, `too_large` and `error` are
/// all built by [`without_lines`], and these are their cases on real
/// repositories. Split by subject, not by line count: the answers *with*
/// lines are in [`tests`].
#[cfg(test)]
#[path = "workspace_git_diff_without_lines_tests.rs"]
mod without_lines_tests;
