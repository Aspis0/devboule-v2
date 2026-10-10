//! One operate call over a live link: create, send, resize, claim,
//! interrupt, permission answer, close, stop, and the provider catalog.
//!
//! Each function maps one local-only `RemoteHost*` frame to the peer's own
//! session frame on the held link — the same trust floor the attach path
//! stands on: the peer's capability grant, the target session's scope and the
//! target daemon's normal request handler decide every answer. Nothing here
//! names a session on this machine, and a refusal comes back as the remote's
//! own typed error with its reason intact.
//!
//! The link's worker owns the transport and calls in here. Nothing else
//! touches a socket for an operate call, and nothing here opens one: a call
//! that cannot be written is refused before a byte leaves.

use std::sync::Arc;
use std::time::{Duration, Instant};

use devboule_protocol::{caps, ClientMessage, DaemonMessage, RemoteHostState};

use super::peer_link::{
    needs_pairing_sentence, offline_sentence, sentence_for, state_for, unsupported_sentence,
};
use super::peer_link_state::{HostLink, LinkAnswer, LinkCommand};
use super::peer_link_worker::{peer_row, LinkSession, PROBE_ID_BASE};
use super::ServerState;
use crate::error::DaemonError;

/// The hello protocol version the far daemon must speak for a file call.
/// The editor frames are new variants an older reader cannot deserialize,
/// so the refusal happens here, before anything is written — the same
/// check the session calls make against their capability.
fn file_edit_advertised(session: &LinkSession) -> bool {
    session.hello.protocol_version >= devboule_protocol::FILE_EDIT_MIN_VERSION
}

/// The hello capability the far daemon must advertise for a session operate
/// call. A daemon that predates the session frames cannot deserialize the
/// request, so the refusal happens here, before anything is written — the
/// same check the attach path makes.
fn sessions_advertised(session: &LinkSession) -> bool {
    session
        .hello
        .capabilities
        .iter()
        .any(|agreed| agreed.as_str() == caps::SESSIONS)
}

/// The two checks every operate call makes before writing: the pairing row is
/// live (a revoke that landed while the link was up stops the next call, not
/// only the next reconnect) and the call was queued against the link that is
/// still there. File calls stop here: they ride no session capability, so
/// the session check below does not apply to them.
fn operate_ready_link(
    state: &Arc<ServerState>,
    link: &HostLink,
    generation: u64,
    answer: &std::sync::mpsc::SyncSender<LinkAnswer>,
) -> bool {
    if let Err(step) = peer_row(state, &link.device_id) {
        let _ = answer.send(LinkAnswer::Failed(state_for(step), sentence_for(step)));
        return false;
    }
    if generation != link.generation() {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Offline,
            offline_sentence().to_string(),
        ));
        return false;
    }
    true
}

/// The two checks above plus the session capability: session calls need a
/// far side that speaks sessions, file calls do not.
fn operate_ready(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &LinkSession,
    generation: u64,
    answer: &std::sync::mpsc::SyncSender<LinkAnswer>,
) -> bool {
    if !operate_ready_link(state, link, generation, answer) {
        return false;
    }
    if !sessions_advertised(session) {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Unsupported,
            unsupported_sentence().to_string(),
        ));
        return false;
    }
    true
}

/// Read frames until the operate call's id answers, a refusal arrives for it,
/// or the deadline passes. A push that lands mid-call (a workspace revision,
/// another stream's event, a keepalive pong) is handled before the call's own
/// answer is matched; it names no request, so it must never be mistaken for
/// the reply. A frame for another id is dropped rather than returned: after a
/// reconnect the remote is one link behind, and its old reply must not be
/// handed to a caller as this call's answer.
fn wait_for_operate_reply(
    link: &HostLink,
    session: &mut LinkSession,
    request_id: u64,
    read_deadline: Duration,
    mut matched: impl FnMut(DaemonMessage) -> Option<LinkAnswer>,
) -> LinkAnswer {
    let deadline = Instant::now() + read_deadline;
    while Instant::now() < deadline {
        // A revoke that lands mid-call stops the wait at once: the pairing
        // is gone, so no answer from this host can be trusted, and holding
        // the caller (up to the whole create budget) would only delay the
        // `NeedsPairing` the row already decides.
        if link.is_revoked() {
            return LinkAnswer::Failed(
                RemoteHostState::NeedsPairing,
                needs_pairing_sentence().to_string(),
            );
        }
        let left = deadline.saturating_duration_since(Instant::now());
        // The socket wait is sliced so a mid-call revoke is observed
        // promptly instead of at the deadline. A quiet slice just loops;
        // a dead transport still fails fast.
        let slice = left.min(Duration::from_millis(50));
        let message = match session.framed.recv_timeout(slice) {
            Ok(message) => message,
            // A quiet slice just loops. Silence has three spellings: the
            // control plane's own timeout, and the socket's timed-out or
            // would-block read — the Noise transport reports a quiet wait
            // either way depending on platform (macOS reports WouldBlock
            // where Windows reports TimedOut). Treating WouldBlock as
            // fatal ends every sliced wait on its first slice there.
            Err(DaemonError::TimedOut(_)) => continue,
            Err(DaemonError::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                continue
            }
            // A revoke is ordered before the teardown it triggers, but a
            // closed socket can still win the race on the way out: check
            // the mark before classifying, so a real revoke is never
            // reported as Offline.
            Err(_) => {
                if link.is_revoked() {
                    return LinkAnswer::Failed(
                        RemoteHostState::NeedsPairing,
                        needs_pairing_sentence().to_string(),
                    );
                }
                return LinkAnswer::Failed(
                    RemoteHostState::Offline,
                    offline_sentence().to_string(),
                );
            }
        };
        if let DaemonMessage::Pong { id, .. } = &message {
            if *id >= PROBE_ID_BASE {
                session.outstanding_since = None;
                session.misses = 0;
                continue;
            }
        }
        super::peer_link_worker::record_workspace_change(
            link,
            session.hello.protocol_version,
            &message,
        );
        if let DaemonMessage::SubscriptionEvent {
            subscription_id,
            envelope,
        } = &message
        {
            link.forward_event(*subscription_id, envelope);
            continue;
        }
        if let DaemonMessage::Error(error) = &message {
            if error.id == Some(request_id) {
                // The remote's own refusal, reason intact.
                return LinkAnswer::Refused(error.clone());
            }
            continue;
        }
        if let Some(answer) = matched(message) {
            return answer;
        }
    }
    LinkAnswer::Failed(RemoteHostState::Offline, offline_sentence().to_string())
}

/// Whether a reply frame answers this call.
fn reply_id(message: &DaemonMessage) -> Option<u64> {
    match message {
        DaemonMessage::Session { id, .. }
        | DaemonMessage::SessionSend { id, .. }
        | DaemonMessage::Providers { id, .. }
        | DaemonMessage::WorkspaceFiles { id, .. }
        | DaemonMessage::WorkspaceGit { id, .. }
        | DaemonMessage::WorkspaceFileOpened { id, .. }
        | DaemonMessage::WorkspaceFileVersion { id, .. }
        | DaemonMessage::WorkspaceFileWrite { id, .. }
        | DaemonMessage::Ok { id } => Some(*id),
        _ => None,
    }
}

/// Create one session on the far side. The caller's idempotency key travels
/// with the peer's own `SessionCreate`, so an explicit retry after a lost
/// reply answers with the same session instead of minting a second one. The
/// worker never retries itself: an unknown outcome stays unknown until the
/// user asks again with the same key.
pub(crate) fn serve_create(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::Create {
        generation,
        workspace_id,
        kind,
        provider,
        mode,
        display_name,
        idempotency_key,
        cols,
        rows,
        answer,
    } = command
    else {
        return;
    };
    if !operate_ready(state, link, session, generation, &answer) {
        return;
    }
    if session
        .framed
        .send(&ClientMessage::SessionCreate {
            id: request_id,
            workspace_id,
            kind,
            provider,
            mode,
            display_name,
            idempotency_key,
            cols,
            rows,
        })
        .is_err()
    {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Offline,
            offline_sentence().to_string(),
        ));
        return;
    }
    let _ = answer.send(wait_for_operate_reply(
        link,
        session,
        request_id,
        read_deadline,
        |message| match message {
            DaemonMessage::Session {
                id,
                session: created,
                ..
            } if id == request_id => Some(LinkAnswer::Created(Box::new(created))),
            _ if reply_id(&message) == Some(request_id) => Some(LinkAnswer::Failed(
                RemoteHostState::Offline,
                offline_sentence().to_string(),
            )),
            _ => None,
        },
    ));
}

/// Send text into one session on the far side.
pub(crate) fn serve_send(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::Send {
        generation,
        session_id,
        subscription_id,
        text,
        attachments,
        active_turn_behavior,
        idempotency_key,
        attachment_references,
        answer,
    } = command
    else {
        return;
    };
    if !operate_ready(state, link, session, generation, &answer) {
        return;
    }
    if session
        .framed
        .send(&ClientMessage::SessionSend {
            id: request_id,
            session_id,
            subscription_id,
            text,
            attachments,
            active_turn_behavior,
            idempotency_key,
            attachment_references,
        })
        .is_err()
    {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Offline,
            offline_sentence().to_string(),
        ));
        return;
    }
    let _ = answer.send(wait_for_operate_reply(
        link,
        session,
        request_id,
        read_deadline,
        |message| match message {
            DaemonMessage::SessionSend { id, turn_active } if id == request_id => {
                Some(LinkAnswer::Sent(turn_active))
            }
            _ if reply_id(&message) == Some(request_id) => Some(LinkAnswer::Failed(
                RemoteHostState::Offline,
                offline_sentence().to_string(),
            )),
            _ => None,
        },
    ));
}

/// One `Ok`-shaped operate call: resize, claim, interrupt, permission answer,
/// close and stop all answer `Ok` on the peer, so they share the wait. The
/// request is built by the caller; this function only carries it.
#[allow(clippy::too_many_arguments)]
fn serve_ok_shaped(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    generation: u64,
    answer: &std::sync::mpsc::SyncSender<LinkAnswer>,
    request: ClientMessage,
    request_id: u64,
    read_deadline: Duration,
) {
    if !operate_ready(state, link, session, generation, answer) {
        return;
    }
    if session.framed.send(&request).is_err() {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Offline,
            offline_sentence().to_string(),
        ));
        return;
    }
    let _ = answer.send(wait_for_operate_reply(
        link,
        session,
        request_id,
        read_deadline,
        |message| match message {
            DaemonMessage::Ok { id } if id == request_id => Some(LinkAnswer::Accepted),
            _ if reply_id(&message) == Some(request_id) => Some(LinkAnswer::Failed(
                RemoteHostState::Offline,
                offline_sentence().to_string(),
            )),
            _ => None,
        },
    ));
}

/// Resize one terminal on the far side.
pub(crate) fn serve_resize(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::Resize {
        generation,
        session_id,
        subscription_id,
        cols,
        rows,
        answer,
    } = command
    else {
        return;
    };
    let request = ClientMessage::SessionResize {
        id: request_id,
        session_id,
        subscription_id,
        cols,
        rows,
    };
    serve_ok_shaped(
        state,
        link,
        session,
        generation,
        &answer,
        request,
        request_id,
        read_deadline,
    );
}

/// Claim one terminal's resize right on the far side.
pub(crate) fn serve_claim(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::Claim {
        generation,
        session_id,
        subscription_id,
        answer,
    } = command
    else {
        return;
    };
    let request = ClientMessage::SessionClaim {
        id: request_id,
        session_id,
        subscription_id,
    };
    serve_ok_shaped(
        state,
        link,
        session,
        generation,
        &answer,
        request,
        request_id,
        read_deadline,
    );
}

/// Interrupt one session on the far side.
pub(crate) fn serve_interrupt(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::Interrupt {
        generation,
        session_id,
        subscription_id,
        answer,
    } = command
    else {
        return;
    };
    let request = ClientMessage::SessionInterrupt {
        id: request_id,
        session_id,
        subscription_id,
    };
    serve_ok_shaped(
        state,
        link,
        session,
        generation,
        &answer,
        request,
        request_id,
        read_deadline,
    );
}

/// Answer one permission card on the far side. The card was created and is
/// resolved on the target host; the answer carries the idempotency key
/// through so a retried answer does not resolve twice.
pub(crate) fn serve_permission_respond(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::PermissionRespond {
        generation,
        session_id,
        subscription_id,
        request_id: card_id,
        outcome,
        option_id,
        answer_text,
        idempotency_key,
        answer,
    } = command
    else {
        return;
    };
    let request = ClientMessage::SessionPermissionRespond {
        id: request_id,
        session_id,
        subscription_id,
        request_id: card_id,
        outcome,
        option_id,
        answer: answer_text,
        idempotency_key,
    };
    serve_ok_shaped(
        state,
        link,
        session,
        generation,
        &answer,
        request,
        request_id,
        read_deadline,
    );
}

/// Close one session on the far side.
pub(crate) fn serve_close(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::Close {
        generation,
        session_id,
        idempotency_key,
        answer,
    } = command
    else {
        return;
    };
    let request = ClientMessage::SessionClose {
        id: request_id,
        session_id,
        idempotency_key,
    };
    serve_ok_shaped(
        state,
        link,
        session,
        generation,
        &answer,
        request,
        request_id,
        read_deadline,
    );
}

/// Stop one session's process on the far side, keeping the session.
pub(crate) fn serve_stop(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::Stop {
        generation,
        session_id,
        subscription_id,
        answer,
    } = command
    else {
        return;
    };
    let request = ClientMessage::SessionStop {
        id: request_id,
        session_id,
        subscription_id,
    };
    serve_ok_shaped(
        state,
        link,
        session,
        generation,
        &answer,
        request,
        request_id,
        read_deadline,
    );
}

/// Switch the model of one session on the far side.
pub(crate) fn serve_set_model(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::SetModel {
        generation,
        session_id,
        model_id,
        effort,
        answer,
    } = command
    else {
        return;
    };
    let request = ClientMessage::SessionSetModel {
        id: request_id,
        session_id,
        model_id,
        effort,
    };
    serve_ok_shaped(
        state,
        link,
        session,
        generation,
        &answer,
        request,
        request_id,
        read_deadline,
    );
}

/// Switch the mode of one session on the far side.
pub(crate) fn serve_set_mode(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::SetMode {
        generation,
        session_id,
        mode_id,
        answer,
    } = command
    else {
        return;
    };
    let request = ClientMessage::SessionSetMode {
        id: request_id,
        session_id,
        mode_id,
    };
    serve_ok_shaped(
        state,
        link,
        session,
        generation,
        &answer,
        request,
        request_id,
        read_deadline,
    );
}

/// Read the far side's provider catalog, for the create picker.
pub(crate) fn serve_providers(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::Providers {
        generation, answer, ..
    } = command
    else {
        return;
    };
    if !operate_ready(state, link, session, generation, &answer) {
        return;
    }
    if session
        .framed
        .send(&ClientMessage::ProvidersList { id: request_id })
        .is_err()
    {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Offline,
            offline_sentence().to_string(),
        ));
        return;
    }
    let _ = answer.send(wait_for_operate_reply(
        link,
        session,
        request_id,
        read_deadline,
        |message| match message {
            DaemonMessage::Providers {
                id,
                providers,
                unreadable_dirs,
            } if id == request_id => Some(LinkAnswer::Providers {
                providers,
                unreadable_dirs,
            }),
            _ if reply_id(&message) == Some(request_id) => Some(LinkAnswer::Failed(
                RemoteHostState::Offline,
                offline_sentence().to_string(),
            )),
            _ => None,
        },
    ));
}

/// Open one file in a workspace on the far side. The paired row is checked
/// live (a revoke that landed while the link was up stops the next call),
/// the far hello must speak the editor dialect, and the peer's `admin`
/// grant decides the answer on the far side exactly as it does for a local
/// open — this side adds no grant of its own.
pub(crate) fn serve_file_open(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::FileOpen {
        generation,
        workspace_id,
        path,
        answer,
    } = command
    else {
        return;
    };
    if !operate_ready_link(state, link, generation, &answer) {
        return;
    }
    if !file_edit_advertised(session) {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Unsupported,
            unsupported_sentence().to_string(),
        ));
        return;
    }
    if session
        .framed
        .send(&ClientMessage::WorkspaceFileOpen {
            id: request_id,
            workspace_id,
            path,
        })
        .is_err()
    {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Offline,
            offline_sentence().to_string(),
        ));
        return;
    }
    let _ = answer.send(wait_for_operate_reply(
        link,
        session,
        request_id,
        read_deadline,
        |message| match message {
            DaemonMessage::WorkspaceFileOpened { id, file, .. } if id == request_id => {
                Some(LinkAnswer::FileOpened(file))
            }
            _ if reply_id(&message) == Some(request_id) => Some(LinkAnswer::Failed(
                RemoteHostState::Offline,
                offline_sentence().to_string(),
            )),
            _ => None,
        },
    ));
}

/// The version of one such file.
pub(crate) fn serve_file_version(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::FileVersion {
        generation,
        workspace_id,
        path,
        answer,
    } = command
    else {
        return;
    };
    if !operate_ready_link(state, link, generation, &answer) {
        return;
    }
    if !file_edit_advertised(session) {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Unsupported,
            unsupported_sentence().to_string(),
        ));
        return;
    }
    if session
        .framed
        .send(&ClientMessage::WorkspaceFileVersion {
            id: request_id,
            workspace_id,
            path,
        })
        .is_err()
    {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Offline,
            offline_sentence().to_string(),
        ));
        return;
    }
    let _ = answer.send(wait_for_operate_reply(
        link,
        session,
        request_id,
        read_deadline,
        |message| match message {
            DaemonMessage::WorkspaceFileVersion { id, version, .. } if id == request_id => {
                Some(LinkAnswer::FileVersion(version))
            }
            _ if reply_id(&message) == Some(request_id) => Some(LinkAnswer::Failed(
                RemoteHostState::Offline,
                offline_sentence().to_string(),
            )),
            _ => None,
        },
    ));
}

/// Write one such file, with the caller's expected version carried through
/// so a retry the user explicitly makes answers like the local write
/// would: `written`, `conflict` with the fresh version, or `error`.
pub(crate) fn serve_file_write(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::FileWrite {
        generation,
        workspace_id,
        path,
        content,
        expected_modified_at,
        expected_revision,
        create,
        answer,
    } = command
    else {
        return;
    };
    if !operate_ready_link(state, link, generation, &answer) {
        return;
    }
    if !file_edit_advertised(session) {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Unsupported,
            unsupported_sentence().to_string(),
        ));
        return;
    }
    if session
        .framed
        .send(&ClientMessage::WorkspaceFileWrite {
            id: request_id,
            workspace_id,
            path,
            content,
            expected_modified_at,
            expected_revision,
            create,
        })
        .is_err()
    {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Offline,
            offline_sentence().to_string(),
        ));
        return;
    }
    let _ = answer.send(wait_for_operate_reply(
        link,
        session,
        request_id,
        read_deadline,
        |message| match message {
            DaemonMessage::WorkspaceFileWrite { id, result, .. } if id == request_id => {
                Some(LinkAnswer::FileWrite(result))
            }
            _ if reply_id(&message) == Some(request_id) => Some(LinkAnswer::Failed(
                RemoteHostState::Offline,
                offline_sentence().to_string(),
            )),
            _ => None,
        },
    ));
}
/// List one directory of a workspace on the far side, for a remote
/// Files panel. A read: the peer's own `WorkspaceFilesList` goes out on
/// the held link and the host's own directory comes back unchanged. No
/// session capability is involved — the far frames predate the editor —
/// so only the link itself is checked.
pub(crate) fn serve_files_list(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::FilesList {
        generation,
        workspace_id,
        path,
        answer,
    } = command
    else {
        return;
    };
    if !operate_ready_link(state, link, generation, &answer) {
        return;
    }
    if session
        .framed
        .send(&ClientMessage::WorkspaceFilesList {
            id: request_id,
            workspace_id,
            path,
        })
        .is_err()
    {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Offline,
            offline_sentence().to_string(),
        ));
        return;
    }
    let _ = answer.send(wait_for_operate_reply(
        link,
        session,
        request_id,
        read_deadline,
        |message| match message {
            DaemonMessage::WorkspaceFiles { id, directory, .. } if id == request_id => {
                Some(LinkAnswer::Files(directory))
            }
            _ if reply_id(&message) == Some(request_id) => Some(LinkAnswer::Failed(
                RemoteHostState::Offline,
                offline_sentence().to_string(),
            )),
            _ => None,
        },
    ));
}

/// The working-tree status of a workspace on the far side, for a remote
/// Changes panel. Read-only by contract: no stage, diff or commit rides
/// this road, so the panel opens file tabs from the rows instead.
pub(crate) fn serve_git_status(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::GitStatus {
        generation,
        workspace_id,
        answer,
    } = command
    else {
        return;
    };
    if !operate_ready_link(state, link, generation, &answer) {
        return;
    }
    if session
        .framed
        .send(&ClientMessage::WorkspaceGitStatus {
            id: request_id,
            workspace_id,
        })
        .is_err()
    {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Offline,
            offline_sentence().to_string(),
        ));
        return;
    }
    let _ = answer.send(wait_for_operate_reply(
        link,
        session,
        request_id,
        read_deadline,
        |message| match message {
            DaemonMessage::WorkspaceGit { id, status, .. } if id == request_id => {
                Some(LinkAnswer::GitStatus(status))
            }
            _ if reply_id(&message) == Some(request_id) => Some(LinkAnswer::Failed(
                RemoteHostState::Offline,
                offline_sentence().to_string(),
            )),
            _ => None,
        },
    ));
}
