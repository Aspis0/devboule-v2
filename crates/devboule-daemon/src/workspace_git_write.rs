//! The Changes panel's write acts over one workspace's repository:
//! stage, unstage, discard and commit — the index and the history, the
//! physically riskiest slice here, while the owner works in the same
//! checkout. Orchestration only: the folder comes from a workspace id
//! (never a request field), every requested path is confined through the
//! **two layers the reads use** — [`confined`] for the spelling and
//! [`walk`] for the filesystem, plus the listing's `.git` guard in any
//! spelling — **before any process is spawned**, then `git` runs through
//! the house runner ([`git`]) under a per-workspace mutex the reply takes
//! (`ServerState::git_write_lock`), so two of this daemon's writers never
//! cross. Against the owner's own git in a terminal the arbiter stays
//! `index.lock`: whoever arrives second loses in milliseconds with no data
//! damage, and the sentence says so ([`write_failure`]).
//!
//! Three invariants state why this file looks the way it does:
//!
//! - **No sentence carries a path or git's stderr.** `error` on these
//!   frames crosses a wire whose redaction seam does not touch it, and
//!   git's stderr holds absolute paths and personal file names — a
//!   failure answers with the operation and its exit code, except the
//!   one collision every owner hits (`index.lock`), matched on stderr
//!   **locally** and answered with the shared static sentence.
//! - **A pathspec never reaches git naked**: `--literal-pathspecs` and
//!   `--` on every path-taking command below, so a file named `-f` is a
//!   file and `a[1].txt` is not a glob (each proved by its own test and
//!   killed by its own mutation: dropping `--` kills the `-f` case,
//!   dropping the literal flag kills the glob case — measured on git
//!   2.54.0).
//! - **The commit is staged-only**: no `add -A` exists in this module
//!   (Paseo's `commitChanges` adds everything by default because it has
//!   no separate stage — this panel does, `DECISIONS-write.md` §2), and
//!   the message is written by hand: empty after trimming is refused
//!   before anything spawns.

use std::path::Path;

use devboule_protocol::DaemonMessage;

use crate::git::{GitOutput, GIT_STDOUT_MAX_BYTES};
use crate::workspace_files::names_git_metadata;
use crate::workspace_git_support::{
    confined, git, probe, run_error, walk, write_failure, Walked, OUTSIDE_THE_WORKSPACE,
};
use crate::ServerState;

/// The workspace's own folder in any spelling (`""`, `"."`, `"./"`): the
/// one pathspec that would silently stage or unstage the whole checkout —
/// the `add -A` this design refuses. Refused before confinement, which
/// would call `""` an escape and walk `"."` to zero components (the
/// walk's own precondition). [`commit`] takes no paths and never sees it.
const THE_ROOT: &str = "the workspace's own folder cannot be staged, unstaged or discarded";
/// Selection cap per request, refused **before any spawn**. The panel
/// sends one row's path per click; the cap bounds a frame from a peer,
/// and it is declared on the wire frame's `paths` field the same 500.
const SELECTION_MAX: usize = 500;
const TOO_MANY_PATHS: &str = "too many paths in one request";
/// Refused before the probe runs — a message judged empty is the caller's
/// own error, and naming it first keeps a non-repository from answering a
/// question that was not asked.
const MESSAGE_EMPTY: &str = "the commit message is empty";
/// Every failure **after** the reset: by then the selection has left the
/// index (through the reset, or through its `rm --cached` fallback — the
/// fallback is reachable inside the discard, measured: an `HEAD` that does
/// not resolve makes `reset` exit 128 and the fallback exit 0), so the
/// sentence admits the stop instead of reporting the failing command as if
/// nothing had happened. The points it covers, each measured on this
/// machine: the classification `status` dying at 128 on an unresolvable
/// `HEAD`; the reply cut — 400 changed paths of ~60 characters produce
/// 22.000 bytes of `status --porcelain=v1 -z`, past the 16 KiB
/// accumulator; and `checkout`/`clean` refusing a file (the case
/// `plan-write.md` §3.3.4 names — the plan's static "the file is in use"
/// sentence is not in this slice, and the exit code would otherwise be
/// the whole answer with the index already moved). One sentence for all
/// four because it claims only what is true at all of them: the discard
/// stopped, and the selection is unstaged. The panel refreshes after
/// every answer, so the list under the toolbar is already the true state
/// and a retry finishes the act. `index.lock` keeps its own sentence one
/// step earlier — inside `reset_or_unindex`, where a double failure means
/// the index never moved.
const DISCARD_HALF_RUN: &str =
    "the discard stopped after unstaging the selection — the panel has re-read it; discard those \
     paths again to finish";

/// Resolve `workspace_id` through the registry — never a request field —
/// take this workspace's write mutex, run one act, and hand back only its
/// refusal (the registry's own sentence names no path; see
/// `workspace_cwd`). One shape for all four frames: the operation the
/// caller sent is the operation the caller knows about.
fn answered(
    state: &ServerState,
    workspace_id: &str,
    act: impl FnOnce(&Path) -> Result<(), String>,
) -> Option<String> {
    match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => {
            let lock = state.git_write_lock(&root);
            let _guard = lock.lock().unwrap_or_else(|error| error.into_inner());
            act(&root).err()
        }
        Err(error) => Some(error.message),
    }
}

/// The reply to [`devboule_protocol::ClientMessage::WorkspaceGitStage`]:
/// nothing on success, the refusing sentence otherwise.
pub(crate) fn reply_stage(
    state: &ServerState,
    id: u64,
    workspace_id: &str,
    paths: &[String],
) -> DaemonMessage {
    let error = answered(state, workspace_id, |root| stage(root, paths));
    DaemonMessage::WorkspaceGitWrite { id, error }
}

/// The reply to [`devboule_protocol::ClientMessage::WorkspaceGitUnstage`].
pub(crate) fn reply_unstage(
    state: &ServerState,
    id: u64,
    workspace_id: &str,
    paths: &[String],
) -> DaemonMessage {
    let error = answered(state, workspace_id, |root| unstage(root, paths));
    DaemonMessage::WorkspaceGitWrite { id, error }
}

/// The reply to [`devboule_protocol::ClientMessage::WorkspaceGitDiscard`].
pub(crate) fn reply_discard(
    state: &ServerState,
    id: u64,
    workspace_id: &str,
    paths: &[String],
) -> DaemonMessage {
    let error = answered(state, workspace_id, |root| discard(root, paths));
    DaemonMessage::WorkspaceGitWrite { id, error }
}

/// The reply to [`devboule_protocol::ClientMessage::WorkspaceGitCommit`].
pub(crate) fn reply_commit(
    state: &ServerState,
    id: u64,
    workspace_id: &str,
    message: &str,
) -> DaemonMessage {
    let error = answered(state, workspace_id, |root| commit(root, message));
    DaemonMessage::WorkspaceGitWrite { id, error }
}

/// Every path of one request, confined through both layers **before any
/// spawn**: the spelling (`confined`), the listing's `.git` guard in any
/// spelling it resolves to, then the walk — a link refused with its own
/// sentence, a missing component allowed because staging a deletion names
/// a file the worktree no longer has (git is the side that decides
/// whether it knows the name). The cap and the root check run first:
/// both are spelling-only and must not cost a stat, let alone a spawn.
fn confine_selection<'a>(root: &Path, paths: &'a [String]) -> Result<Vec<&'a str>, String> {
    if paths.len() > SELECTION_MAX {
        return Err(TOO_MANY_PATHS.to_string());
    }
    let mut confined_paths = Vec::with_capacity(paths.len());
    for requested in paths {
        if Path::new(requested)
            .components()
            .all(|component| matches!(component, std::path::Component::CurDir))
        {
            // Also covers `""`, whose component list is already empty.
            return Err(THE_ROOT.to_string());
        }
        if confined(root, requested).is_none() {
            return Err(OUTSIDE_THE_WORKSPACE.to_string());
        }
        if names_git_metadata(requested) {
            return Err(crate::workspace_files::NOT_PART_OF_THE_TREE.to_string());
        }
        match walk(root, requested) {
            Walked::Link(sentence) => return Err(sentence.to_string()),
            Walked::Missing | Walked::Inside(_) => {}
        }
        confined_paths.push(requested.as_str());
    }
    Ok(confined_paths)
}

/// One write command under this house's discipline: closed argv through
/// [`git`], and on failure the operation's sentence — never stderr. The
/// caller's `prefix` carries the flags (`--literal-pathspecs`, `--`), so
/// a path can never be parsed as an option.
fn write(root: &Path, operation: &str, arguments: &[&str]) -> Result<GitOutput, String> {
    let output = git(root, arguments).map_err(|error| run_error(error, operation))?;
    if output.success {
        Ok(output)
    } else {
        Err(write_failure(operation, &output))
    }
}

/// Stage the selection: a tracked file's change, a new file, or a tracked
/// file's deletion — `git add` covers the three — under literal pathspecs
/// and `--`. An empty selection is a no-op success (Paseo's
/// `discardChanges` returns the same way for nothing selected).
fn stage(root: &Path, paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    // Confinement first and without a process: nothing below spawns — not
    // even the probe — for a path this check refused.
    let selected = confine_selection(root, paths)?;
    if let Some(sentence) = probe(root).refusal() {
        return Err(sentence.to_string());
    }
    let mut arguments = vec!["--literal-pathspecs", "add", "--"];
    arguments.extend(selected);
    write(root, "git add", &arguments).map(|_| ())
}

/// The reset the unstage and the discard both open with — and its
/// declared fallback. `git reset HEAD -- <paths>` is the road, and when
/// it fails (measured: an `HEAD` that does not resolve exits 128, while
/// git 2.54 exits **0** on a merely unborn branch) the paths come out of
/// the index directly, Paseo's fallback (`checkout-git.ts:3516`), which
/// needs no `HEAD` at all. A failure of the fallback itself is reported
/// as its own command's sentence.
fn reset_or_unindex(root: &Path, selected: &[&str]) -> Result<(), String> {
    let mut reset = vec!["--literal-pathspecs", "reset", "-q", "HEAD", "--"];
    reset.extend_from_slice(selected);
    if write(root, "git reset", &reset).is_ok() {
        return Ok(());
    }
    let mut fallback = vec![
        "--literal-pathspecs",
        "rm",
        "--cached",
        "-r",
        "-q",
        "--ignore-unmatch",
        "--",
    ];
    fallback.extend_from_slice(selected);
    write(root, "git rm --cached", &fallback).map(|_| ())
}

/// Unstage the selection: the index entry goes back to `HEAD` and the
/// worktree keeps its bytes — the act that loses nothing, which is why it
/// asks no confirmation (`DECISIONS-write.md` §1).
fn unstage(root: &Path, paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let selected = confine_selection(root, paths)?;
    if let Some(sentence) = probe(root).refusal() {
        return Err(sentence.to_string());
    }
    reset_or_unindex(root, &selected)
}

/// Discard the selection — the one act here that **loses data**, hence
/// the sending screen's own `confirm()` gate (the wire carries none; the
/// panel asks before it calls, `useWorkspaceGitActions`). Paseo's
/// sequence, pathspec-scoped at every step (`checkout-git.ts:3503-3567`):
/// unstage everything, classify the result, restore tracked paths from
/// the index (= `HEAD` after the reset) and delete untracked ones. The
/// classification runs **after** the reset on purpose — load-bearing:
/// a staged new file is `A ` before it and `??` after, and only the
/// second classification deletes it (Paseo's three truths,
/// `checkout-git.test.ts:3827`). From the reset onward every failure
/// answers [`DISCARD_HALF_RUN`]: the index has moved by then, and a bare
/// exit code would say nothing about that.
fn discard(root: &Path, paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let selected = confine_selection(root, paths)?;
    if let Some(sentence) = probe(root).refusal() {
        return Err(sentence.to_string());
    }
    reset_or_unindex(root, &selected)?;
    let mut status = vec![
        "--literal-pathspecs",
        "--no-optional-locks",
        "status",
        "--porcelain=v1",
        "-z",
        "--",
    ];
    status.extend_from_slice(&selected);
    let status = write(root, "git status", &status).map_err(|_| DISCARD_HALF_RUN.to_string())?;
    // The accumulator cuts at `GIT_STDOUT_MAX_BYTES + 1`: past that the
    // classification is a guess, and guessing which files to restore or
    // delete is how half a selection vanishes. The reset above already
    // ran — the sentence says exactly that much and nothing false.
    if status.stdout.len() > GIT_STDOUT_MAX_BYTES {
        return Err(DISCARD_HALF_RUN.to_string());
    }
    let (tracked, untracked) = classify(&status.stdout);
    if !tracked.is_empty() {
        let mut checkout = vec!["--literal-pathspecs", "checkout", "-q", "--"];
        checkout.extend_from_slice(&tracked);
        write(root, "git checkout", &checkout).map_err(|_| DISCARD_HALF_RUN.to_string())?;
    }
    if !untracked.is_empty() {
        let mut clean = vec!["--literal-pathspecs", "clean", "-fd", "-q", "--"];
        clean.extend_from_slice(&untracked);
        write(root, "git clean", &clean).map_err(|_| DISCARD_HALF_RUN.to_string())?;
    }
    Ok(())
}

/// A `-z` `--porcelain=v1` dump split into the paths `checkout` takes and
/// the paths `clean` takes: `?? ` is untracked, everything else is
/// tracked, and a rename/copy entry carries its original path as the very
/// next NUL token — skipped by position, the only thing stopping that
/// bare second token from being classified as a path of its own
/// (Paseo's own step, `checkout-git.ts:3540-3544`). Borrowed from
/// `stdout`, which the caller still holds.
fn classify(stdout: &str) -> (Vec<&str>, Vec<&str>) {
    let mut tracked = Vec::new();
    let mut untracked = Vec::new();
    let tokens: Vec<&str> = stdout.split('\0').collect();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        index += 1;
        // `XY <path>`: two status bytes, one space, then the path. A
        // shorter token is a trailing empty split, never a path.
        if token.len() < 4 {
            continue;
        }
        let state = &token[..2];
        let path = &token[3..];
        if state == "??" {
            untracked.push(path);
        } else {
            tracked.push(path);
        }
        if state.starts_with('R') || state.starts_with('C') {
            index += 1;
        }
    }
    (tracked, untracked)
}

/// Commit **what is staged and nothing else**: no `add -A` exists here,
/// the message is the caller's own (empty after trimming refused before
/// the probe runs), and a hook that dies answers as operation plus exit
/// code — measured on this machine `git commit` reports a failed
/// `pre-commit` as its own exit **1** while the hook's stderr (paths and
/// all) stays dropped. `--` closes the argv although no path follows: no
/// path-taking argument exists on this command, and the flag costs
/// nothing (measured on an unborn branch as well).
fn commit(root: &Path, message: &str) -> Result<(), String> {
    if message.trim().is_empty() {
        return Err(MESSAGE_EMPTY.to_string());
    }
    if let Some(sentence) = probe(root).refusal() {
        return Err(sentence.to_string());
    }
    write(
        root,
        "git commit",
        &["--literal-pathspecs", "commit", "-m", message, "--"],
    )
    .map(|_| ())
}

#[cfg(test)]
#[path = "workspace_git_write_tests.rs"]
mod tests;
