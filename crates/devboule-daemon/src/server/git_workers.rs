//! The request side of the workspace git arms off the connection loop: the
//! arms that classify as git-backed, the queue keys they resolve, the read
//! keys and sinks coalescing joins on, and the `offload` that moves a
//! request onto its root's queue. The queue itself — lanes, permits, caps,
//! the write-drain — is the substrate in [`super::git_queue`], which knows
//! nothing about `ClientMessage`.
//!
//! The arms run off the loop because `dispatch` runs on the connection's
//! loop thread — the same thread that writes replies and drains session
//! events — and every arm [`is_git_backed`] admits runs git there: behind a
//! 10 s probe, a workspace arm for up to several 60 s-capped commands,
//! `ProjectAdd` for a 10 s probe alone. One slow repository would hold the
//! whole channel: every other RPC and every session event queues behind it.
//!
//! Three properties the single-shot provider workers do not need:
//!
//! - **Per-root order.** One queue per resolved repository root is drained
//!   in arrival order, so stage-then-commit and commit-then-log keep the
//!   order the app issued them in, and a keyed write's check-then-act is
//!   serialized exactly as it was on the loop. Two ids on one checkout share
//!   one root key, like `git_write_lock`; worktree add and remove serialize
//!   through the registry's `worktree_creation` serial, which the delete
//!   also holds, because a create keys on the project while a delete keys
//!   on the checkout — deliberately different queues.
//! - **Lanes.** Reads (status, diff, log) share a four-permit lane; writes
//!   run on their own two-permit lane, so a write never queues behind
//!   another root's read sweep. Within a root, a read whose lane has no
//!   permit to take steps aside for the root's own writes, for as long as
//!   they keep arriving — the starvation that costs is accepted, because a
//!   poll is re-issued on its timer and a person's commit is not re-issued
//!   at all. The write runs first and the read then observes it — fresher
//!   than the tree the read was queued against, never staler. A read
//!   queued after a write never runs before it.
//! - **Bounded queues.** At most `QUEUED_READ_CAP` reads queue per root and
//!   at most `WAITING_READ_CAP` more wait on the root's waitlist — admitted
//!   as reads free their queued slots, joined when the same read repeats,
//!   and past the waitlist's cap the newest read supersedes the oldest,
//!   whose sinks are told so. Writes queue in order and are never capped:
//!   every mutation must run.
//! - **Coalesced polls.** A read may join this root's **last** queued job
//!   when it is the same read — never across a write, whose result the
//!   poll must postdate — or the identical read on the waitlist, and
//!   every joined request is answered from that job's result with its own
//!   id. Writes never coalesce.
//! - **Shutdown.** A write the queue accepted before shutdown — a delete
//!   among them — completes, journal row included, if it finishes inside
//!   the 10 s drain bound; a write still queued when the bound runs out is
//!   answered ShuttingDown and does nothing. The Shutdown arm itself only
//!   checkpoints the journal (a flush, never a close): the one terminal
//!   close runs on the shutdown path, after that drain.
//!
//! One behavioural seam is left deliberately: a still-inline arm that reads
//! the tree right after a queued git write no longer orders after it.
//! Nothing in the shipped frontend does that — post-write refreshes go back
//! through the git frames — and the queue exists precisely because those
//! writes no longer finish before the next frame is dispatched.

use std::sync::Arc;

use devboule_protocol::WorkspaceIsolation;

use super::git_queue::{Job, ReadKey, Sink};
use super::*;

/// Opaque read-key tags; the substrate compares keys for equality only.
const STATUS_READ: u64 = 1;
const DIFF_READ: u64 = 2;
const LOG_READ: u64 = 3;

/// The arms whose handling runs git. Written by "can reach git", not by
/// "always spawns it": a Local `WorkspaceDelete` refuses before any git
/// runs, and admitting it costs that refusal one queue hop and nothing else.
pub(super) fn is_git_backed(request: &ClientMessage) -> bool {
    matches!(
        request,
        ClientMessage::WorkspaceGitStatus { .. }
            | ClientMessage::WorkspaceGitDiff { .. }
            | ClientMessage::WorkspaceGitLog { .. }
            | ClientMessage::WorkspaceGitStage { .. }
            | ClientMessage::WorkspaceGitUnstage { .. }
            | ClientMessage::WorkspaceGitDiscard { .. }
            | ClientMessage::WorkspaceGitCommit { .. }
            | ClientMessage::WorkspaceFileRename { .. }
            | ClientMessage::WorkspaceDelete { .. }
            | ClientMessage::WorkspaceCreate {
                isolation: WorkspaceIsolation::Worktree,
                ..
            }
            | ClientMessage::ProjectAdd { .. }
    )
}

/// What a queued read would be a repeat of, `None` for every write. The
/// diff's path is part of the key: two files' diffs are not the same read.
fn poll_key(request: &ClientMessage) -> Option<ReadKey> {
    match request {
        ClientMessage::WorkspaceGitStatus { .. } => Some(ReadKey::new(STATUS_READ, None)),
        ClientMessage::WorkspaceGitDiff { path, .. } => {
            Some(ReadKey::new(DIFF_READ, Some(path.clone())))
        }
        ClientMessage::WorkspaceGitLog { .. } => Some(ReadKey::new(LOG_READ, None)),
        _ => None,
    }
}

/// The interception [`super::dispatch`] routes [`is_git_backed`] arms
/// through: move the request onto this root's queue and answer nothing here
/// — the job enqueues the reply. The queue key is the workspace's resolved
/// root where the request names a workspace (two ids on one checkout share
/// one queue, like `git_write_lock`), the canonical project path in the
/// same plain spelling for a project add — so two spellings of one folder,
/// and the folder's own workspace frames, share one queue; an unresolvable
/// path never reaches git and shares one key so it cannot grow the map —
/// and the project for a worktree create. Only arms [`is_git_backed`]
/// admitted reach this match.
fn queue_key(state: &ServerState, request: &ClientMessage) -> String {
    match request {
        ClientMessage::WorkspaceGitStatus { workspace_id, .. }
        | ClientMessage::WorkspaceGitDiff { workspace_id, .. }
        | ClientMessage::WorkspaceGitLog { workspace_id, .. }
        | ClientMessage::WorkspaceGitStage { workspace_id, .. }
        | ClientMessage::WorkspaceGitUnstage { workspace_id, .. }
        | ClientMessage::WorkspaceGitDiscard { workspace_id, .. }
        | ClientMessage::WorkspaceGitCommit { workspace_id, .. }
        | ClientMessage::WorkspaceFileRename { workspace_id, .. }
        | ClientMessage::WorkspaceDelete { workspace_id, .. } => state
            .sessions
            .workspace_cwd(workspace_id)
            .map(|root| root.to_string_lossy().into_owned())
            // Per id: such a request produces the arm's own refusal, and
            // that sentence names the workspace — one shared fallback
            // queue would let a joined read deliver it to a different id.
            .unwrap_or_else(|_| format!("workspace:{workspace_id}")),
        ClientMessage::WorkspaceCreate { project_id, .. } => format!("project:{project_id}"),
        ClientMessage::ProjectAdd { path, .. } => crate::workspace::canonical_directory(path)
            .map(|root| crate::verbatim_path::plain_path(&root.to_string_lossy()))
            // Shared: this arm is a write, so nothing coalesces on the key,
            // and one key for every unresolvable path keeps client input
            // from growing the never-evicting map.
            .unwrap_or_else(|_| String::from("project-add-unresolvable")),
        _ => String::from("workspace:unkeyed"),
    }
}

/// A read's answer, re-stamped for a different request id. The three read
/// arms answer with these three shapes and no others — their refusals ride
/// inside the payload's own `error` field — plus, from `dispatch_immediate`,
/// a plain `Error` (the shutdown refusal); anything else is unreachable, and
/// the deliverer replaces it with a plain error rather than a
/// mis-addressed frame.
fn restamp(reply: &DaemonMessage, id: u64) -> Option<DaemonMessage> {
    match reply {
        DaemonMessage::WorkspaceGit { status, .. } => Some(DaemonMessage::WorkspaceGit {
            id,
            status: status.clone(),
        }),
        DaemonMessage::WorkspaceGitLog { log, .. } => Some(DaemonMessage::WorkspaceGitLog {
            id,
            log: log.clone(),
        }),
        DaemonMessage::WorkspaceGitFile { file, .. } => Some(DaemonMessage::WorkspaceGitFile {
            id,
            file: file.clone(),
        }),
        DaemonMessage::Error(error) => {
            let error = error.clone().with_id(id);
            Some(DaemonMessage::Error(error))
        }
        _ => None,
    }
}

/// The ProvidersRefresh shape, plus the queue: build the job, hand it to the
/// queue, and on a failed start answer the error the other workers answer.
/// The ninth parameter is the peer-gate token `run_gate` minted for this
/// request; it moves into the job so the answer is produced by the same
/// gated road [`super::dispatch`] would have taken. The list is dispatch's
/// eight parameters plus that token, hence the allow.
#[allow(clippy::too_many_arguments)]
pub(super) fn offload(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    request: ClientMessage,
    conn: &Arc<ConnHandle>,
    passed: GatePassed,
    sessions_ok: bool,
    journal_ok: bool,
    typed_permissions_ok: bool,
    devices_ok: bool,
) -> Option<DaemonMessage> {
    let request_id = request.request_id();
    let trace_name = request.name();
    let root = queue_key(state, &request);
    let read_key = poll_key(&request);
    // A write accepted before the flag went up runs to its end even when it
    // is popped after shutdown started; the drain-bound cancel answers the
    // rest. dispatch refuses new requests on the loop, so this capture is
    // the belt to that suspenders.
    let queued_write = read_key.is_none() && !state.is_shutting_down();
    let enqueued = Instant::now();
    let worker_state = Arc::clone(state);
    let worker_owner = owner.clone();
    let worker_conn = Arc::clone(conn);
    let outbound = Arc::clone(&conn.outbound);
    let failure_outbound = Arc::clone(&outbound);
    let cancel_state = Arc::clone(state);
    let id = request_id.unwrap_or_default();
    let answer = move |request: ClientMessage| {
        let started = Instant::now();
        let queue_wait_ms = started
            .checked_duration_since(enqueued)
            .unwrap_or_default()
            .as_millis()
            .to_string();
        // A panicked act must still answer the caller and must not strand
        // the queue: either would hang every later request for this
        // workspace.
        let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dispatch_immediate(
                &worker_state,
                &worker_owner,
                request,
                &worker_conn,
                sessions_ok,
                journal_ok,
                typed_permissions_ok,
                devices_ok,
                &passed,
                queued_write,
            )
        }))
        .unwrap_or_else(|_| {
            DaemonMessage::Error(
                WireError::new(ErrorCode::Io, "the workspace git request failed").with_id(id),
            )
        });
        let took_ms = started.elapsed().as_millis().to_string();
        crate::rpc_trace::daemon_event(
            "dispatch_worker_end",
            trace_name,
            request_id,
            worker_conn.id,
            &[
                ("took_ms", took_ms.as_str()),
                ("queue_wait_ms", queue_wait_ms.as_str()),
            ],
        );
        reply
    };
    let job = if let Some(key) = read_key {
        let sinks = Arc::new(std::sync::Mutex::new(vec![Sink::new(Box::new(
            move |reply| {
                let reply = restamp(reply, id).unwrap_or_else(|| {
                    DaemonMessage::Error(
                        WireError::new(ErrorCode::Io, "the workspace git request failed")
                            .with_id(id),
                    )
                });
                outbound.enqueue_reply(reply);
            },
        ))]));
        Job::Read {
            key,
            compute: Box::new({
                let answer = answer;
                move || answer(request)
            }),
            sinks,
        }
    } else {
        Job::Write(Box::new(move || {
            // The drain bound expired with this write still queued: the
            // flush it was meant to precede has gone by, so it is answered
            // instead of run.
            if cancel_state.git_jobs.writes_cancelled() {
                let error =
                    WireError::new(ErrorCode::ShuttingDown, "daemon is shutting down").with_id(id);
                outbound.enqueue_reply(DaemonMessage::Error(error));
                return;
            }
            let reply = answer(request);
            outbound.enqueue_reply(reply);
        }))
    };
    if let Err(reason) = state.git_jobs.enqueue_job(root, job) {
        if let Some(id) = request_id {
            failure_outbound.enqueue_reply(DaemonMessage::Error(
                WireError::new(ErrorCode::Io, reason).with_id(id),
            ));
        }
    }
    None
}

/// Open to `crate::server` so `crate::server::tests` can name the wait inside by full path.
#[cfg(test)]
#[path = "git_workers_test_support.rs"]
pub(super) mod test_support;

#[cfg(test)]
#[path = "git_workers_order_tests.rs"]
mod order_tests;

#[cfg(test)]
#[path = "git_workers_bound_tests.rs"]
mod bound_tests;

#[cfg(test)]
#[path = "git_workers_join_tests.rs"]
mod join_tests;

#[cfg(test)]
#[path = "git_workers_delete_tests.rs"]
mod delete_tests;

#[cfg(test)]
#[path = "git_workers_archive_mark_tests.rs"]
mod archive_mark_tests;

#[cfg(test)]
#[path = "git_workers_shutdown_tests.rs"]
mod shutdown_tests;

#[cfg(test)]
#[path = "git_workers_drain_bound_tests.rs"]
mod drain_bound_tests;

#[cfg(test)]
#[path = "git_workers_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "git_workers_lanes_tests.rs"]
mod lanes_tests;

#[cfg(test)]
#[path = "git_workers_waitlist_tests.rs"]
mod waitlist_tests;
