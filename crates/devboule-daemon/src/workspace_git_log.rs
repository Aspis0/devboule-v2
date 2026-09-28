//! The commit history of one workspace, for the Changes panel's Commits
//! section: the branch's own commits newest-first (at most 200 of them),
//! then at most ten of the base branch's recent history, beside the base
//! ref the split was computed against. Orchestration only: the folder comes
//! from a workspace id, every git command runs through the house runner under
//! its timeouts and stdout cap, and the bytes are handed to [`parse`].
//!
//! Translated from Paseo's `listCheckoutCommits` (Apache-2.0, Copyright
//! (c) 2025-present Mohamed Boudra,
//! `packages/server/src/utils/checkout-git.ts`), modified for this daemon.

use std::collections::HashSet;
use std::path::Path;

use devboule_protocol::{DaemonMessage, WorkspaceGitCommitEntry, WorkspaceGitLog};

use crate::workspace_git_support::{exit_error, git, git_with_cap, probe, run_error};
use crate::ServerState;

#[path = "workspace_git_log_base.rs"]
mod base;
#[path = "workspace_git_log_parse.rs"]
mod parse;

/// How many of the base branch's commits the reply carries — the recent
/// history at and before the fork point, newest first. The workspace's own
/// list is capped separately ([`WORKSPACE_COMMIT_LIMIT`]): the shared
/// stdout accumulator bounds it, and a run that passes the cap answers
/// with the records it parsed and the truncation sentence rather than an
/// empty list.
const BASE_COMMIT_LIMIT: usize = 10;

/// The workspace half's own commit limit, applied in git: the Commits
/// section shows recent history, and the limit keeps the read's traversal
/// (and its bytes) bounded. Paseo's workspace half is uncapped; this daemon
/// caps it so the reply's size is computable.
const WORKSPACE_COMMIT_LIMIT: usize = 200;

/// A generous per-record size: the header git's `--format` writes per
/// commit — sha, short sha, author, ISO date, subject — is a few hundred
/// bytes at a realistic subject, and this is several times that.
const LOG_RECORD_MAX_BYTES: usize = 1024;

/// The log read's own accumulator ceiling: the two commit limits git
/// applies, times the generous per-record size. The shared 16 KiB ceiling
/// bounds the status read, whose output is the working tree; the log read's
/// output is the history, which no working-tree bound covers.
const LOG_STDOUT_MAX_BYTES: usize =
    (WORKSPACE_COMMIT_LIMIT + BASE_COMMIT_LIMIT) * LOG_RECORD_MAX_BYTES;

/// The `--format` of one record: record-separated, NUL-field-separated, so
/// a subject carrying any byte but a NUL stays parseable. `%x1e`/`%x00`
/// are git placeholders — literal text in the argument, real bytes in the
/// output; a real NUL cannot travel inside a process argument.
const COMMIT_LOG_FORMAT: &str = "--format=%x1e%H%x00%h%x00%an%x00%aI%x00%s";

/// Resolve `workspace_id` through the registry — never a request field —
/// and read the workspace's commit history from that folder.
pub(crate) fn reply(state: &ServerState, id: u64, workspace_id: &str) -> DaemonMessage {
    let log = match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => match state.sessions.workspace_branch(workspace_id) {
            // The stored base is the branch this workspace was cut from —
            // a worktree workspace's own record, this daemon's worktree
            // metadata. `None` for a Local workspace.
            Ok(branch) => log_of(&root, branch.as_deref()),
            Err(error) => unavailable(error.message),
        },
        Err(error) => unavailable(error.message),
    };
    DaemonMessage::WorkspaceGitLog { id, log }
}

/// The history of one directory. Private on purpose: callers outside this
/// module arrive through [`reply`], and the test modules are children.
fn log_of(root: &Path, stored_base_ref: Option<&str>) -> WorkspaceGitLog {
    // The probe's own sentence for every answer that is not `Ready` —
    // "not a repository" and "inside a repository" are classifications
    // the status read carries on `is_git`; this reply has no such field,
    // so the shared sentence travels in `error`.
    if let Some(message) = probe(root).refusal() {
        return build(None, Vec::new(), Some(message.to_string()));
    }
    // The stored base is persisted verbatim; a name beginning with `-`
    // would reach git's argv as an option. Paseo's `assertSafeGitRef`
    // (`packages/server/src/server/worktree-session.ts`) rejects this value
    // class for the same reason, and the check runs here — before any argv is
    // built.
    if let Some(sentence) = stored_base_ref.and_then(unsafe_base_ref) {
        return build(None, Vec::new(), Some(sentence.to_string()));
    }
    // A detached HEAD outside a rebase, and a repository with no commit
    // yet, have no branch to split from: the answer is an empty history,
    // not a refusal (Paseo's `getCurrentBranch` returns null there).
    let Some(current_branch) = base::current_branch(root) else {
        return build(None, Vec::new(), None);
    };
    let resolved_base_ref = match stored_base_ref {
        Some(stored) => Some(stored.to_string()),
        None => match base::repository_default_branch(root) {
            Ok(default) => default,
            Err(sentence) => return build(None, Vec::new(), Some(sentence)),
        },
    };
    let normalized_base_ref = resolved_base_ref
        .as_deref()
        .and_then(base::branch_name_from_ref);
    let mut comparison_base_ref = match base::try_resolve_comparison_base_ref(
        root,
        resolved_base_ref.as_deref(),
        &current_branch,
    ) {
        Ok(comparison) => comparison,
        Err(sentence) => return build(None, Vec::new(), Some(sentence)),
    };
    if comparison_base_ref.is_none()
        && normalized_base_ref.is_some_and(|name| !name.is_empty() && name != current_branch)
    {
        // The saved base can outlive a renamed or deleted base branch: the
        // retry runs against the repository's default branch, Paseo's own
        // fallback (`resolveBaseRef`).
        if let Ok(Some(default)) = base::repository_default_branch(root) {
            if let Ok(comparison) =
                base::try_resolve_comparison_base_ref(root, Some(&default), &current_branch)
            {
                comparison_base_ref = comparison;
            }
        }
    }

    let mut workspace_records = Vec::new();
    let mut base_revision = String::from("HEAD");
    let mut truncated = false;
    if let Some(comparison) = comparison_base_ref.as_deref() {
        let run = match commit_records(
            root,
            &format!("{comparison}..HEAD"),
            Some(WORKSPACE_COMMIT_LIMIT),
        ) {
            Ok(run) => run,
            Err(sentence) => return build(None, Vec::new(), Some(sentence)),
        };
        // The workspace half's limit is a truncation: a branch longer than
        // the limit ships its newest commits and the flag.
        truncated = run.byte_truncated || run.limit_hit;
        workspace_records = run.commits;
        base_revision = merge_base(root, comparison).unwrap_or_default();
    }
    let base_records = if base_revision.is_empty() {
        Vec::new()
    } else {
        let run = match commit_records(root, &base_revision, Some(BASE_COMMIT_LIMIT)) {
            Ok(run) => run,
            Err(sentence) => return build(None, Vec::new(), Some(sentence)),
        };
        // The base half's limit is the intended recent-history cap, not a
        // truncation: a base branch longer than ten commits is ordinary, and
        // the list of ten is the complete answer.
        truncated = truncated || run.byte_truncated;
        run.commits
    };
    // The workspace half's own shas, taken before the base half joins them:
    // a commit is base history exactly when the workspace's list did not
    // carry it (Paseo's `isOnBase`).
    let workspace_shas: HashSet<String> = workspace_records
        .iter()
        .map(|record| record.sha.clone())
        .collect();
    let mut records = workspace_records;
    records.extend(base_records);
    // A cut-short list is flagged, not dropped: the records parsed up to the
    // last complete one are the newest commits, and the sentence says the
    // oldest are missing. `base_ref` stands — the split was computed
    // before the read ran, the way the status read's `branch` survives its
    // own truncation. One sentence for both cut-short causes (the byte
    // cap and the commit limit): the reply's cap, whichever of the two it
    // was that the branch outgrew.
    let truncation = truncated.then(|| {
        "git log produced more than the reply cap; the oldest commits are missing from this list"
            .to_string()
    });
    if records.is_empty() {
        return build(comparison_base_ref, Vec::new(), truncation);
    }
    let unpushed_shas = match unpushed_shas(root) {
        Ok(shas) => shas,
        Err(sentence) => return build(None, Vec::new(), Some(sentence)),
    };
    let commits = records
        .into_iter()
        .map(|record| {
            let is_on_remote = !unpushed_shas.contains(&record.sha);
            let is_on_base = !workspace_shas.contains(&record.sha);
            WorkspaceGitCommitEntry {
                sha: record.sha,
                short_sha: record.short_sha,
                subject: record.subject,
                author_name: record.author_name,
                author_date: record.author_date,
                is_on_remote,
                is_on_base,
            }
        })
        .collect();
    build(comparison_base_ref, commits, truncation)
}

/// Assemble the reply.
fn build(
    base_ref: Option<String>,
    commits: Vec<WorkspaceGitCommitEntry>,
    error: Option<String>,
) -> WorkspaceGitLog {
    WorkspaceGitLog {
        base_ref,
        commits,
        error,
    }
}

/// No answer at all: no commits and `error` saying why.
fn unavailable(message: impl Into<String>) -> WorkspaceGitLog {
    build(None, Vec::new(), Some(message.into()))
}

/// A base ref that would reach git's argv as an option: git parses any
/// leading `-` as one, and the stored base branch is persisted verbatim.
/// The sentence is static — a ref name is not a filesystem path, but the
/// house rule is that `error` carries no path-like text, and the helper
/// that enforces it cannot tell a ref's `/` from a path's.
fn unsafe_base_ref(base_ref: &str) -> Option<&'static str> {
    base_ref
        .starts_with('-')
        .then_some("the stored base branch is not a valid git ref")
}

/// One `git log` run's outcome: the parsed records, and the two ways the
/// run can be cut short. `byte_truncated` is the accumulator's cap;
/// `limit_hit` is git returning more records than the limit asked for —
/// whether that is a truncation worth flagging is the caller's: the
/// workspace half's limit is a truncation, the base half's is the intended
/// recent-history cap.
struct LogRun {
    commits: Vec<parse::Commit>,
    byte_truncated: bool,
    limit_hit: bool,
}

/// The parsed commit records of one `git log` run — the branch's commits
/// (`<base>..HEAD`, capped at [`WORKSPACE_COMMIT_LIMIT`]) or the base's
/// recent history (`<revision>`, capped at [`BASE_COMMIT_LIMIT`]). The
/// commit limit is applied in git, not after reading; the byte cap is the
/// log read's own ([`LOG_STDOUT_MAX_BYTES`]), sized to those limits.
fn commit_records(root: &Path, revision: &str, max_count: Option<usize>) -> Result<LogRun, String> {
    // Ask git for one more than the limit: a full answer then means the
    // limit cut the history, and the extra record is the flag — the same
    // disclosure the byte cap makes, at the count threshold. Without it a
    // branch longer than the limit would render a list that looks complete.
    let max_count_argument = max_count.map(|max| format!("--max-count={}", max + 1));
    // `--end-of-options` before the revision: the revision is derived from
    // the stored base branch, and without the separator git parses a
    // leading `-` as its own option (measured: `git log --output=x..HEAD`
    // writes the log to a file named `x..HEAD` and exits 0). The separator
    // makes an option-looking revision an operand git refuses (exit 128)
    // instead of a flag it applies.
    let arguments: Vec<&str> = match max_count_argument.as_deref() {
        Some(max) => vec![
            "--no-optional-locks",
            "log",
            max,
            "--diff-merges=first-parent",
            COMMIT_LOG_FORMAT,
            "--end-of-options",
            revision,
        ],
        None => vec![
            "--no-optional-locks",
            "log",
            "--diff-merges=first-parent",
            COMMIT_LOG_FORMAT,
            "--end-of-options",
            revision,
        ],
    };
    let output = git_with_cap(root, &arguments, LOG_STDOUT_MAX_BYTES)
        .map_err(|error| run_error(error, "git log"))?;
    if !output.success {
        // An old git that does not know `--end-of-options` (git ≥ 2.24,
        // Nov 2019) exits 129 with an "unknown option" error — mapped to
        // the version sentence, because "exited with code 129" is a code
        // no user can act on. Nothing else in the argv can be unknown:
        // every other option predates 2.24 by years.
        if output.code == Some(129) && output.stderr.to_lowercase().contains("unknown option") {
            return Err("git log needs git 2.24 or newer".to_string());
        }
        return Err(exit_error("git log", &output));
    }
    let mut commits = parse::parse_commit_records(&output.stdout);
    // The accumulator stops at `LOG_STDOUT_MAX_BYTES + 1`, so anything
    // longer was cut — the records then end at the last complete one.
    let byte_truncated = output.stdout.len() > LOG_STDOUT_MAX_BYTES;
    let mut limit_hit = false;
    if let Some(max) = max_count {
        if commits.len() > max {
            commits.truncate(max);
            limit_hit = true;
        }
    }
    Ok(LogRun {
        commits,
        byte_truncated,
        limit_hit,
    })
}

/// The fork point of `base_ref` and HEAD — where the workspace's history
/// diverged, and the commit the base list starts at. `None` when git cannot
/// name one; the base list then starts at HEAD itself (Paseo's
/// `mergeBase ?? ""`).
fn merge_base(root: &Path, base_ref: &str) -> Option<String> {
    let output = git(
        root,
        &[
            "--no-optional-locks",
            "merge-base",
            "--end-of-options",
            base_ref,
            "HEAD",
        ],
    )
    .ok()?;
    let sha = output.stdout.trim();
    (!sha.is_empty()).then(|| sha.to_string())
}

/// Every commit git can reach from HEAD that no remote ref can — the
/// local-only (unpushed) half of `isOnRemote`. Paseo's
/// `getUnpushedCommitShas`.
fn unpushed_shas(root: &Path) -> Result<HashSet<String>, String> {
    let output = git(
        root,
        &[
            "--no-optional-locks",
            "rev-list",
            "HEAD",
            "--not",
            "--remotes",
        ],
    )
    .map_err(|error| run_error(error, "git rev-list"))?;
    if !output.success {
        return Err(exit_error("git rev-list", &output));
    }
    Ok(output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
#[path = "workspace_git_log_tests.rs"]
mod tests;
