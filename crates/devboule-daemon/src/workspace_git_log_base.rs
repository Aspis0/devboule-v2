//! Which ref to split a workspace's history at — the base-ref resolution
//! half of the log read. The stored base (a worktree workspace's own
//! record), the repository's default branch behind it, the local-vs-origin
//! heuristic for a bare name, and the merge-target decision a qualified
//! ref gets. Every function is a pure fact about one directory and git:
//! no reply is composed here, and the orchestration that consumes these
//! answers lives in `workspace_git_log.rs`.
//!
//! Translated from Paseo's `getCurrentBranch` (with its rebase-head
//! fallback), `branchNameFromRef`, `resolveRepositoryDefaultBranch`,
//! `doesGitRefExist`, `isQualifiedBranchRef`, `resolveMostAheadBaseRef`
//! and `tryResolveCheckoutCommitsBaseRef` (Apache-2.0, Copyright (c)
//! 2025-present Mohamed Boudra,
//! `packages/server/src/utils/checkout-git.ts`), modified for this daemon.

use std::collections::HashSet;
use std::path::Path;

use crate::workspace_git_support::{exit_error, git, run_error};

/// The branch HEAD sits on, or `None` when it does not — a detached HEAD
/// outside a rebase, a repository with no commit yet, or git refusing to
/// answer. A rebase keeps its branch: git records it in
/// `.git/rebase-merge/head-name` (or `rebase-apply`), and the source's
/// `getCurrentBranch` reads the same two paths.
pub(super) fn current_branch(root: &Path) -> Option<String> {
    let output = git(
        root,
        &["--no-optional-locks", "rev-parse", "--abbrev-ref", "HEAD"],
    )
    .ok()?;
    if !output.success {
        return None;
    }
    let branch = output.stdout.trim();
    if branch == "HEAD" {
        return rebase_branch(root);
    }
    (!branch.is_empty()).then(|| branch.to_string())
}

/// The branch a detached HEAD is rebasing, read from git's own rebase
/// state files. `None` when there is no rebase — a plain detached HEAD has
/// no branch.
fn rebase_branch(root: &Path) -> Option<String> {
    for path in ["rebase-merge/head-name", "rebase-apply/head-name"] {
        let Ok(output) = git(
            root,
            &["--no-optional-locks", "rev-parse", "--git-path", path],
        ) else {
            continue;
        };
        if !output.success {
            continue;
        }
        let Ok(name) = std::fs::read_to_string(root.join(output.stdout.trim())) else {
            continue;
        };
        let name = name.trim();
        let branch = name.strip_prefix("refs/heads/").unwrap_or(name);
        if !branch.is_empty() {
            return Some(branch.to_string());
        }
    }
    None
}

/// The branch name behind a ref — display and legacy identity only; it
/// cannot round-trip, so anything that has to resolve to a commit keeps
/// the exact ref instead. The source's `branchNameFromRef`.
pub(super) fn branch_name_from_ref(reference: &str) -> Option<&str> {
    let trimmed = reference.trim();
    if let Some(rest) = trimmed.strip_prefix("refs/heads/") {
        return Some(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("refs/remotes/") {
        // Slashes are everywhere in branch names and rare in remote names,
        // so the first segment after `refs/remotes/` is read as the remote.
        return match rest.find('/') {
            Some(separator) => Some(&rest[separator + 1..]),
            None => Some(rest),
        };
    }
    if let Some(rest) = trimmed.strip_prefix("origin/") {
        return Some(rest);
    }
    (!trimmed.is_empty()).then_some(trimmed)
}

/// The repository's default branch: `origin/HEAD` when it points at one
/// (the local name when a local branch of that name exists, the remote
/// spelling otherwise), else `main`, else `master`. The source's
/// `resolveRepositoryDefaultBranch` — the symbolic-ref half is best-effort
/// and falls through to the listing on any failure; the listing itself is
/// the authority whose failure is a refusal.
pub(super) fn repository_default_branch(root: &Path) -> Result<Option<String>, String> {
    let origin_head = git(
        root,
        &[
            "--no-optional-locks",
            "symbolic-ref",
            "--quiet",
            "refs/remotes/origin/HEAD",
        ],
    )
    .ok()
    .filter(|output| output.success);
    if let Some(output) = origin_head {
        let reference = output.stdout.trim();
        if !reference.is_empty() {
            let remote_short = reference.strip_prefix("refs/remotes/").unwrap_or(reference);
            let local_name = remote_short.strip_prefix("origin/").unwrap_or(remote_short);
            // The existence check rides the same best-effort fall-through as
            // the symbolic-ref itself: the source's catch swallows it, and the
            // branch listing below is the authority.
            let local_exists =
                reference_exists(root, &format!("refs/heads/{local_name}")).unwrap_or(false);
            return Ok(Some(if local_exists {
                local_name.to_string()
            } else {
                remote_short.to_string()
            }));
        }
    }
    let output = git(
        root,
        &["--no-optional-locks", "branch", "--format=%(refname:short)"],
    )
    .map_err(|error| run_error(error, "git branch"))?;
    if !output.success {
        return Err(exit_error("git branch", &output));
    }
    let branches: HashSet<&str> = output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if branches.contains("main") {
        Ok(Some("main".to_string()))
    } else if branches.contains("master") {
        Ok(Some("master".to_string()))
    } else {
        Ok(None)
    }
}

/// Whether `reference` names a ref git can resolve — `show-ref --verify`,
/// whose exit 1 is an answer (absent), not a failure. The source's
/// `doesGitRefExist`.
fn reference_exists(root: &Path, reference: &str) -> Result<bool, String> {
    let output = git(
        root,
        &[
            "--no-optional-locks",
            "show-ref",
            "--verify",
            "--quiet",
            reference,
        ],
    )
    .map_err(|error| run_error(error, "git show-ref"))?;
    Ok(output.success)
}

/// A fully qualified branch ref (`refs/heads/…`, `refs/remotes/…`) names
/// the exact commit stream to compare against; a bare name keeps the
/// local-vs-origin heuristic below. The source's `isQualifiedBranchRef`.
fn is_qualified_branch_ref(reference: &str) -> bool {
    reference.starts_with("refs/heads/") || reference.starts_with("refs/remotes/")
}

/// The ref to list workspace commits against for one base name: the local
/// branch, the origin branch, or whichever is ahead when both exist. A
/// qualified ref is verified as-is — a caller who named an exact ref meant
/// it, so its absence is the caller's error. The source's
/// `resolveMostAheadBaseRef`.
fn resolve_most_ahead_base_ref(root: &Path, base_ref: &str) -> Result<String, String> {
    if is_qualified_branch_ref(base_ref) {
        if reference_exists(root, base_ref)? {
            return Ok(base_ref.to_string());
        }
        // The sentence names no ref: a git ref is not a filesystem path,
        // but the house rule is that `error` carries no path-like text,
        // and the helper that enforces it cannot tell a ref's `/` from a
        // path's.
        return Err("the base ref does not exist".to_string());
    }
    let name = branch_name_from_ref(base_ref).unwrap_or(base_ref);
    let has_local = reference_exists(root, &format!("refs/heads/{name}"))?;
    let has_origin = reference_exists(root, &format!("refs/remotes/origin/{name}"))?;
    if has_local && !has_origin {
        return Ok(name.to_string());
    }
    if !has_local && has_origin {
        return Ok(format!("origin/{name}"));
    }
    if !has_local && !has_origin {
        return Err(format!(
            "the base branch {name} is not local and not on origin"
        ));
    }
    let output = git(
        root,
        &[
            "--no-optional-locks",
            "rev-list",
            "--left-right",
            "--count",
            &format!("{name}...origin/{name}"),
        ],
    )
    .map_err(|error| run_error(error, "git rev-list"))?;
    if !output.success {
        return Err(exit_error("git rev-list", &output));
    }
    let parse = |value: Option<&str>| value.and_then(|field| field.parse::<u64>().ok());
    let mut counts = output.stdout.split_whitespace();
    let local_only = parse(counts.next());
    let origin_only = parse(counts.next());
    match (local_only, origin_only) {
        (Some(local), Some(origin)) if origin > local => Ok(format!("origin/{name}")),
        // Unparseable counts fall back to the local name, the source's own
        // answer when git's output is not two numbers.
        _ => Ok(name.to_string()),
    }
}

/// The ref to split the history at, or `None` when the resolved base IS the
/// current branch (nothing to split) or cannot be resolved. A failure on
/// a bare name falls back to `None` — the caller then tries the
/// repository's default branch; a failure on a qualified ref is the
/// caller's error to answer, because a caller who named an exact ref
/// meant it. The source's `tryResolveCheckoutCommitsBaseRef`.
pub(super) fn try_resolve_comparison_base_ref(
    root: &Path,
    resolved_base_ref: Option<&str>,
    current_branch: &str,
) -> Result<Option<String>, String> {
    let Some(base_ref) = resolved_base_ref else {
        return Ok(None);
    };
    let normalized = branch_name_from_ref(base_ref);
    if !normalized.is_some_and(|name| !name.is_empty() && name != current_branch) {
        return Ok(None);
    }
    match resolve_most_ahead_base_ref(root, base_ref) {
        Ok(comparison) => Ok(Some(comparison)),
        Err(error) => {
            if is_qualified_branch_ref(base_ref) {
                Err(error)
            } else {
                Ok(None)
            }
        }
    }
}
