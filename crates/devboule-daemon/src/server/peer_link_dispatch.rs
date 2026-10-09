//! The three local-wire frames that reach the link manager, and the replies
//! that go back out through the connection's own writer.
//!
//! Everything here runs on the request's own worker thread, never on the
//! connection's reader: a read waits on a peer, and a reader that waits on a
//! peer is the deadlock `RECON-client-handler-deadlock.md` records. The reply
//! is queued like every other worker reply, so the writer stays the only thing
//! that touches the socket.

use std::sync::Arc;

use devboule_protocol::{ClientMessage, DaemonMessage, ErrorCode, RemoteHostState, WireError};

use super::peer_link::{busy_sentence, MAX_HELD_LINKS};
use super::peer_link_state::LinkAnswer;
use super::ServerState;
use crate::session::ConnHandle;

/// Serve one `RemoteHost*` request. Any other message is a routing bug and is
/// refused as one rather than reaching the manager.
pub(super) fn dispatch_remote_host(
    state: &Arc<ServerState>,
    conn: &Arc<ConnHandle>,
    request: ClientMessage,
) -> DaemonMessage {
    /// The app-only door every host frame passes: a peer connection —
    /// which is what an agent's cross-machine path resolves to — cannot
    /// open, drive or close a stream through this daemon, and neither can
    /// a handle the daemon built for itself (an agent's local tool call
    /// resolves to one: `mcp_broker::caller` builds it with no kernel
    /// identity). Human-originated requests arrive on the local app
    /// connection only — a real pipe client with a kernel identity, which
    /// the accept layer already verified; agent-originated cross-machine
    /// commands travel the permission-card path instead, and reach these
    /// frames through no tool at all.
    fn app_only(conn: &Arc<ConnHandle>, id: u64) -> Option<DaemonMessage> {
        if conn.conn_peer.is_some() || conn.peer.is_none() {
            return Some(DaemonMessage::Error(
                WireError::new(
                    ErrorCode::InvalidRequest,
                    "a peer cannot operate a remote host through this daemon",
                )
                .with_id(id),
            ));
        }
        None
    }
    /// One operate answer back to the app: the host's own body, or its own
    /// refusal with reason intact, or the link's state sentence when the
    /// link could not carry the call.
    fn operate_answer(
        id: u64,
        device_id: &str,
        answer: LinkAnswer,
        map: impl FnOnce(u64, &str, LinkAnswer) -> Option<DaemonMessage>,
    ) -> DaemonMessage {
        match answer {
            LinkAnswer::Refused(error) => DaemonMessage::Error(error.with_id(id)),
            LinkAnswer::Failed(state, sentence) => DaemonMessage::Error(
                WireError::new(
                    if state == RemoteHostState::Busy {
                        ErrorCode::OperationConflict
                    } else {
                        ErrorCode::Io
                    },
                    sentence,
                )
                .with_id(id),
            ),
            other => map(id, device_id, other).unwrap_or_else(|| {
                DaemonMessage::Error(
                    WireError::new(ErrorCode::Internal, "the host answered out of kind")
                        .with_id(id),
                )
            }),
        }
    }
    match request {
        ClientMessage::RemoteHostWatch { id, device_id } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            let sentence = busy_sentence(MAX_HELD_LINKS, "held links");
            match state.peer_links.watch(state, Arc::clone(conn), &device_id) {
                Ok(()) => DaemonMessage::Ok { id },
                // Refused before a socket or a thread exists. The watching
                // connections are told `busy` too, so the row says why it is
                // empty rather than showing nothing at all.
                Err(()) => {
                    conn.outbound
                        .enqueue_reply(DaemonMessage::RemoteHostStatus {
                            device_id: device_id.clone(),
                            state: RemoteHostState::Busy,
                            last_failure: Some(sentence.clone()),
                            revision: None,
                        });
                    DaemonMessage::Error(
                        WireError::new(ErrorCode::OperationConflict, sentence).with_id(id),
                    )
                }
            }
        }
        ClientMessage::RemoteHostUnwatch { id, device_id } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            // Absent a lease this is still an `Ok`: a panel that tears down
            // twice must not have to be careful about the second time.
            state.peer_links.unwatch(conn.id, &device_id);
            DaemonMessage::Ok { id }
        }
        ClientMessage::RemoteHostAttach {
            id,
            device_id,
            session_id,
            subscription_id,
        } => {
            // Local-only by construction (peer_policy denies it to every cap
            // set); the check here is the belt to that suspenders, and it
            // also stops the daemon's own synthetic handles (an agent's
            // local tool call).
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            match state.peer_links.attach(
                &device_id,
                &session_id,
                subscription_id,
                Arc::clone(conn),
            ) {
                LinkAnswer::Accepted => DaemonMessage::Ok { id },
                LinkAnswer::Refused(error) => DaemonMessage::Error(error.with_id(id)),
                LinkAnswer::Failed(state, sentence) => DaemonMessage::Error(
                    WireError::new(
                        if state == RemoteHostState::Busy {
                            ErrorCode::OperationConflict
                        } else {
                            ErrorCode::Io
                        },
                        sentence,
                    )
                    .with_id(id),
                ),
                LinkAnswer::Body(_) => DaemonMessage::Error(
                    WireError::new(ErrorCode::Internal, "an attach is not a list read").with_id(id),
                ),
                LinkAnswer::Created(_) | LinkAnswer::Sent(_) | LinkAnswer::Providers { .. } => {
                    DaemonMessage::Error(
                        WireError::new(ErrorCode::Internal, "an attach is not an operate call")
                            .with_id(id),
                    )
                }
            }
        }
        ClientMessage::RemoteHostDetach {
            id,
            device_id,
            session_id,
            subscription_id,
        } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            match state
                .peer_links
                .detach(&device_id, &session_id, subscription_id)
            {
                LinkAnswer::Accepted => DaemonMessage::Ok { id },
                LinkAnswer::Refused(error) => DaemonMessage::Error(error.with_id(id)),
                LinkAnswer::Failed(state, sentence) => DaemonMessage::Error(
                    WireError::new(
                        if state == RemoteHostState::Busy {
                            ErrorCode::OperationConflict
                        } else {
                            ErrorCode::Io
                        },
                        sentence,
                    )
                    .with_id(id),
                ),
                LinkAnswer::Body(_) => DaemonMessage::Error(
                    WireError::new(ErrorCode::Internal, "a detach is not a list read").with_id(id),
                ),
                LinkAnswer::Created(_) | LinkAnswer::Sent(_) | LinkAnswer::Providers { .. } => {
                    DaemonMessage::Error(
                        WireError::new(ErrorCode::Internal, "a detach is not an operate call")
                            .with_id(id),
                    )
                }
            }
        }
        ClientMessage::RemoteHostList {
            id,
            device_id,
            list,
        } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            match state.peer_links.read(&device_id, list) {
                LinkAnswer::Body(body) => DaemonMessage::RemoteHostList {
                    id,
                    device_id,
                    body,
                },
                LinkAnswer::Accepted => DaemonMessage::Error(
                    WireError::new(ErrorCode::Internal, "a list read is not an attach").with_id(id),
                ),
                // The remote's own refusal: its code and reason travel back intact,
                // so the host's empty state says what the far machine said.
                LinkAnswer::Refused(error) => DaemonMessage::Error(error.with_id(id)),
                LinkAnswer::Failed(state, sentence) => DaemonMessage::Error(
                    WireError::new(
                        if state == RemoteHostState::Busy {
                            ErrorCode::OperationConflict
                        } else {
                            ErrorCode::Io
                        },
                        sentence,
                    )
                    .with_id(id),
                ),
                LinkAnswer::Created(_) | LinkAnswer::Sent(_) | LinkAnswer::Providers { .. } => {
                    DaemonMessage::Error(
                        WireError::new(ErrorCode::Internal, "a list read is not an operate call")
                            .with_id(id),
                    )
                }
            }
        }
        ClientMessage::RemoteHostCreate {
            id,
            device_id,
            workspace_id,
            kind,
            provider,
            mode,
            display_name,
            idempotency_key,
            cols,
            rows,
        } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            let answer = state.peer_links.operate_create(
                &device_id,
                workspace_id,
                kind,
                provider,
                mode,
                display_name,
                idempotency_key,
                cols,
                rows,
            );
            operate_answer(
                id,
                &device_id,
                answer,
                |id, device_id, answer| match answer {
                    LinkAnswer::Created(session) => Some(DaemonMessage::RemoteHostSession {
                        id,
                        device_id: device_id.to_string(),
                        session: *session,
                    }),
                    _ => None,
                },
            )
        }
        ClientMessage::RemoteHostSend {
            id,
            device_id,
            session_id,
            subscription_id,
            text,
            attachments,
            active_turn_behavior,
            idempotency_key,
            attachment_references,
        } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            let answer = state.peer_links.operate_send(
                &device_id,
                session_id,
                subscription_id,
                text,
                attachments,
                active_turn_behavior,
                idempotency_key,
                attachment_references,
            );
            operate_answer(
                id,
                &device_id,
                answer,
                |id, device_id, answer| match answer {
                    LinkAnswer::Sent(turn_active) => Some(DaemonMessage::RemoteHostSent {
                        id,
                        device_id: device_id.to_string(),
                        turn_active,
                    }),
                    _ => None,
                },
            )
        }
        ClientMessage::RemoteHostResize {
            id,
            device_id,
            session_id,
            subscription_id,
            cols,
            rows,
        } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            let answer = state.peer_links.operate_resize(
                &device_id,
                session_id,
                subscription_id,
                cols,
                rows,
            );
            operate_answer(id, &device_id, answer, |id, _, answer| match answer {
                LinkAnswer::Accepted => Some(DaemonMessage::Ok { id }),
                _ => None,
            })
        }
        ClientMessage::RemoteHostClaim {
            id,
            device_id,
            session_id,
            subscription_id,
        } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            let answer = state
                .peer_links
                .operate_claim(&device_id, session_id, subscription_id);
            operate_answer(id, &device_id, answer, |id, _, answer| match answer {
                LinkAnswer::Accepted => Some(DaemonMessage::Ok { id }),
                _ => None,
            })
        }
        ClientMessage::RemoteHostInterrupt {
            id,
            device_id,
            session_id,
            subscription_id,
        } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            let answer =
                state
                    .peer_links
                    .operate_interrupt(&device_id, session_id, subscription_id);
            operate_answer(id, &device_id, answer, |id, _, answer| match answer {
                LinkAnswer::Accepted => Some(DaemonMessage::Ok { id }),
                _ => None,
            })
        }
        ClientMessage::RemoteHostPermissionRespond {
            id,
            device_id,
            session_id,
            subscription_id,
            request_id,
            outcome,
            option_id,
            answer: answer_text,
            idempotency_key,
        } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            let answer = state.peer_links.operate_permission_respond(
                &device_id,
                session_id,
                subscription_id,
                request_id,
                outcome,
                option_id,
                answer_text,
                idempotency_key,
            );
            operate_answer(id, &device_id, answer, |id, _, answer| match answer {
                LinkAnswer::Accepted => Some(DaemonMessage::Ok { id }),
                _ => None,
            })
        }
        ClientMessage::RemoteHostClose {
            id,
            device_id,
            session_id,
            idempotency_key,
        } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            let answer = state
                .peer_links
                .operate_close(&device_id, session_id, idempotency_key);
            operate_answer(id, &device_id, answer, |id, _, answer| match answer {
                LinkAnswer::Accepted => Some(DaemonMessage::Ok { id }),
                _ => None,
            })
        }
        ClientMessage::RemoteHostStop {
            id,
            device_id,
            session_id,
            subscription_id,
        } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            let answer = state
                .peer_links
                .operate_stop(&device_id, session_id, subscription_id);
            operate_answer(id, &device_id, answer, |id, _, answer| match answer {
                LinkAnswer::Accepted => Some(DaemonMessage::Ok { id }),
                _ => None,
            })
        }
        ClientMessage::RemoteHostProviders { id, device_id } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            let answer = state.peer_links.operate_providers(&device_id);
            operate_answer(
                id,
                &device_id,
                answer,
                |id, device_id, answer| match answer {
                    LinkAnswer::Providers {
                        providers,
                        unreadable_dirs,
                    } => Some(DaemonMessage::RemoteHostProviders {
                        id,
                        device_id: device_id.to_string(),
                        providers,
                        unreadable_dirs,
                    }),
                    _ => None,
                },
            )
        }
        ClientMessage::RemoteHostSetModel {
            id,
            device_id,
            session_id,
            model_id,
            effort,
        } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            let answer = state
                .peer_links
                .operate_set_model(&device_id, session_id, model_id, effort);
            operate_answer(id, &device_id, answer, |id, _, answer| match answer {
                LinkAnswer::Accepted => Some(DaemonMessage::Ok { id }),
                _ => None,
            })
        }
        ClientMessage::RemoteHostSetMode {
            id,
            device_id,
            session_id,
            mode_id,
        } => {
            if let Some(refused) = app_only(conn, id) {
                return refused;
            }
            let answer = state
                .peer_links
                .operate_set_mode(&device_id, session_id, mode_id);
            operate_answer(id, &device_id, answer, |id, _, answer| match answer {
                LinkAnswer::Accepted => Some(DaemonMessage::Ok { id }),
                _ => None,
            })
        }
        other => DaemonMessage::Error(WireError::new(
            ErrorCode::InvalidRequest,
            format!("{} is not a remote-host frame", other.name()),
        )),
    }
}

/// Whether a request is one this module serves. Kept beside the router so the
/// dispatch loop and this file cannot disagree about the set.
pub(super) fn is_remote_host(request: &ClientMessage) -> bool {
    matches!(
        request,
        ClientMessage::RemoteHostWatch { .. }
            | ClientMessage::RemoteHostUnwatch { .. }
            | ClientMessage::RemoteHostList { .. }
            | ClientMessage::RemoteHostAttach { .. }
            | ClientMessage::RemoteHostDetach { .. }
            | ClientMessage::RemoteHostCreate { .. }
            | ClientMessage::RemoteHostSend { .. }
            | ClientMessage::RemoteHostResize { .. }
            | ClientMessage::RemoteHostClaim { .. }
            | ClientMessage::RemoteHostInterrupt { .. }
            | ClientMessage::RemoteHostPermissionRespond { .. }
            | ClientMessage::RemoteHostClose { .. }
            | ClientMessage::RemoteHostStop { .. }
            | ClientMessage::RemoteHostProviders { .. }
            | ClientMessage::RemoteHostSetModel { .. }
            | ClientMessage::RemoteHostSetMode { .. }
            | ClientMessage::RemoteHostFileOpen { .. }
            | ClientMessage::RemoteHostFileVersion { .. }
            | ClientMessage::RemoteHostFileWrite { .. }
    )
}
