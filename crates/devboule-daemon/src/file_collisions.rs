//! Who else is changing this path: every other checkout of the caller's own
//! repository, what each of them has committed since the branch point, and
//! what each has left uncommitted in its working tree.
//!
//! The reads are the ones git already speaks — `worktree list --porcelain`,
//! `merge-base`, `diff --name-only`, `status --porcelain` — behind this
//! house's runner (a closed argv, no shell), one command per worktree and
//! four commands per path in total, so the sweep is bounded three ways: by
//! the number of worktrees ([`MAX_OTHER_WORKTREES`]), by each command's own
//! deadline ([`COMMAND_TIMEOUT`]) and by the sweep's ([`SCAN_DEADLINE`]).
//! Whatever bound the sweep hit is reported as [`CollisionScan::capped`]
//! rather than passed off as a complete answer.
//!
//! The caller passes the root and a path already confined by
//! [`confine_subject`], the same two layers every other reader of a workspace
//! folder uses: the spelling, then a filesystem walk that refuses a link
//! before anything is spawned. Git is never given a path it did not accept
//! here, and never a bare pathspec that a file named `-f` could become.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::git::{GitOutput, GitRunError};
use crate::workspace_files::{names_git_metadata, NOT_PART_OF_THE_TREE};
use crate::workspace_git_support::{
    confined, exit_error, run_error, walk, Walked, OUTSIDE_THE_WORKSPACE,
};
use crate::worktree::{parse_worktree_list_porcelain, path_is_within, ExistingWorktree};

/// Other checkouts one call reads. A repository with more worktrees than this
/// is rare and the overflow is declared in the answer, never hidden.
pub(crate) const MAX_OTHER_WORKTREES: usize = 8;
/// The ceiling on any one command of the sweep. Four commands per worktree at
/// the house's 60 s would be minutes of one agent's turn; a repository slow
/// enough to need more has said it cannot answer in useful time.
pub(crate) const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
/// The ceiling on the whole sweep, checked before each worktree. It is what
/// keeps the per-command bound honest once the worktree cap is not the one
/// that bites.
const SCAN_DEADLINE: Duration = Duration::from_secs(20);

/// A checkout this daemon knows by name: the workspace row whose folder it
/// is, so a row in the answer can be addressed with a workspace id instead of
/// a bare branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KnownCheckout {
    pub path: PathBuf,
    pub workspace_id: String,
}

/// One other checkout of the same repository, as the collision tool reports
/// it. `base_sha` is the branch point with the caller's own `HEAD`; a pair of
/// histories with no common ancestor reports `None` rather than a base, and
/// therefore no committed change — there is no point to compare against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OtherWorktree {
    pub workspace_id: Option<String>,
    pub branch: Option<String>,
    pub base_sha: Option<String>,
    pub committed_change: bool,
    pub dirty_change: bool,
}

/// What the sweep found, and whether it looked at everything there was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CollisionScan {
    pub worktrees: Vec<OtherWorktree>,
    pub capped: bool,
}

/// The path a collision call may ask about, in the spelling both consumers
/// match on (the writer log's [`crate::write_evidence::spelling`], which git
/// also accepts): inside `root` in the two layers the readers share, and never
/// the repository's own metadata folder. The folder itself (`.`, `./`) is a
/// legal subject — "is anyone else changing anything in this repository" is a
/// real question.
pub(crate) fn confine_subject(root: &Path, requested: &str) -> Result<String, &'static str> {
    if confined(root, requested).is_none() {
        return Err(crate::workspace_git_support::OUTSIDE_THE_WORKSPACE);
    }
    if names_git_metadata(requested) {
        return Err(NOT_PART_OF_THE_TREE);
    }
    match walk(root, requested) {
        Walked::Link(sentence) => Err(sentence),
        Walked::Missing | Walked::Inside(_) => {
            crate::write_evidence::spelling(requested).ok_or(OUTSIDE_THE_WORKSPACE)
        }
    }
}

/// Read every other live checkout of `caller_root`'s repository for one
/// already-confined relative path. `run` is the git runner the caller injects
/// (the house's, under [`COMMAND_TIMEOUT`]); the seam is what lets the bounds
/// be tested without a repository of twenty worktrees.
pub(crate) fn scan<F>(
    caller_root: &Path,
    subject: &str,
    known: &[KnownCheckout],
    run: &F,
) -> Result<CollisionScan, String>
where
    F: Fn(&Path, &[&str]) -> Result<GitOutput, GitRunError>,
{
    let listed = read_required(
        caller_root,
        &["worktree", "list", "--porcelain"],
        "git worktree list",
        run,
    )?;
    // One past the cap, so a repository with more checkouts than the sweep
    // reads is known to be one instead of merely looking like a complete list.
    let others: Vec<ExistingWorktree> = parse_worktree_list_porcelain(&listed)
        .into_iter()
        // A bare repository has no checkout and a prunable row's folder is
        // gone; neither holds a file that can collide with this one.
        .filter(|entry| !entry.is_bare && !entry.is_prunable && entry.path.is_dir())
        .filter(|entry| !same_folder(&entry.path, caller_root))
        .take(MAX_OTHER_WORKTREES + 1)
        .collect();
    let capped = others.len() > MAX_OTHER_WORKTREES;
    let caller_head = read_optional(caller_root, &["rev-parse", "HEAD"], "git rev-parse", run)?
        .as_deref()
        .and_then(sha);
    let deadline = Instant::now() + SCAN_DEADLINE;
    let mut worktrees = Vec::with_capacity(others.len().min(MAX_OTHER_WORKTREES));
    for entry in others.iter().take(MAX_OTHER_WORKTREES) {
        if Instant::now() >= deadline {
            return Ok(CollisionScan {
                worktrees,
                capped: true,
            });
        }
        worktrees.push(read_worktree(
            caller_root,
            caller_head.as_deref(),
            subject,
            entry,
            known,
            run,
        )?);
    }
    Ok(CollisionScan { worktrees, capped })
}

/// One checkout's two answers, read against the branch point with the
/// caller's `HEAD`. A worktree with no commit yet (`HEAD` unresolved) reports
/// no base and no committed change, which is true: there is nothing to
/// compare — and its working tree is still read, because an unborn branch can
/// already be dirty.
fn read_worktree<F>(
    caller_root: &Path,
    caller_head: Option<&str>,
    subject: &str,
    entry: &ExistingWorktree,
    known: &[KnownCheckout],
    run: &F,
) -> Result<OtherWorktree, String>
where
    F: Fn(&Path, &[&str]) -> Result<GitOutput, GitRunError>,
{
    let head = read_optional(&entry.path, &["rev-parse", "HEAD"], "git rev-parse", run)?
        .as_deref()
        .and_then(sha);
    let base = match (caller_head, head.as_deref()) {
        (Some(caller), Some(other)) => read_optional(
            caller_root,
            &["merge-base", caller, other],
            "git merge-base",
            run,
        )?
        .as_deref()
        .and_then(sha),
        // No common ancestor is `merge-base`'s own answer (exit 1, empty
        // stdout), already `None` above; a missing HEAD is the other case.
        _ => None,
    };
    let committed_change = match (base.as_deref(), head.as_deref()) {
        (Some(base), Some(head)) => !read_required(
            &entry.path,
            &[
                "--literal-pathspecs",
                "diff",
                "--name-only",
                base,
                head,
                "--",
                subject,
            ],
            "git diff",
            run,
        )?
        .trim()
        .is_empty(),
        _ => false,
    };
    let dirty_change = !read_required(
        &entry.path,
        &[
            "--literal-pathspecs",
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "--",
            subject,
        ],
        "git status",
        run,
    )?
    .is_empty();
    Ok(OtherWorktree {
        workspace_id: known
            .iter()
            .find(|checkout| same_folder(&checkout.path, &entry.path))
            .map(|checkout| checkout.workspace_id.clone()),
        branch: entry.branch.clone(),
        base_sha: base,
        committed_change,
        dirty_change,
    })
}

/// One repository read whose command must have worked: a `git` that timed
/// out or could not be started, and a command that exited non-zero, are both
/// the sweep's failure — answered with the operation and its exit code, never
/// git's stderr, which carries this machine's absolute paths.
fn read_required<F>(
    root: &Path,
    arguments: &[&str],
    operation: &str,
    run: &F,
) -> Result<String, String>
where
    F: Fn(&Path, &[&str]) -> Result<GitOutput, GitRunError>,
{
    let output = run(root, arguments).map_err(|error| run_error(error, operation))?;
    if !output.success {
        return Err(exit_error(operation, &output));
    }
    Ok(output.stdout)
}

/// One repository read that git is allowed to answer "no" to: an unresolvable
/// `HEAD`, or two histories with no branch point, is `None` rather than a
/// failure — while a runner that never answered is still an error, because a
/// silent `false` down the sweep would read as "nobody changed it".
fn read_optional<F>(
    root: &Path,
    arguments: &[&str],
    operation: &str,
    run: &F,
) -> Result<Option<String>, String>
where
    F: Fn(&Path, &[&str]) -> Result<GitOutput, GitRunError>,
{
    let output = run(root, arguments).map_err(|error| run_error(error, operation))?;
    Ok(output.success.then_some(output.stdout))
}

/// Whether two folders are the same one, under the platform's own rule:
/// case-insensitively on Windows, byte-for-byte elsewhere. Worktree rows and
/// workspace rows spell the same folder through different code paths.
fn same_folder(left: &Path, right: &Path) -> bool {
    path_is_within(left, right) && path_is_within(right, left)
}

/// A 40-hex object name, or nothing. `git rev-parse HEAD` on an unborn branch
/// prints the literal word `HEAD` and exits **0**, so the shape is the only
/// thing separating a commit from a ref name git echoed back.
fn sha(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (trimmed.len() == 40 && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| trimmed.to_ascii_lowercase())
}

/// The sweep's bounds and its answers when there is no repository to read —
/// the worktree cap and an unborn `HEAD` — against a runner that answers from
/// a table, so neither needs a repository of its own.
#[cfg(test)]
#[path = "file_collisions_bounds_tests.rs"]
mod bounds_tests;
#[cfg(test)]
#[path = "file_collisions_fixture.rs"]
mod fixture;
#[cfg(test)]
#[path = "file_collisions_tests.rs"]
mod tests;
