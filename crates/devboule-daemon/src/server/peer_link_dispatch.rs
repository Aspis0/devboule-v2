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
    match request {
        ClientMessage::RemoteHostWatch { id, device_id } => {
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
            // set); the check here is the belt to that suspenders.
            if conn.conn_peer.is_some() {
                return DaemonMessage::Error(
                    WireError::new(
                        ErrorCode::InvalidRequest,
                        "a peer cannot open a stream through this daemon",
                    )
                    .with_id(id),
                );
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
            }
        }
        ClientMessage::RemoteHostDetach {
            id,
            device_id,
            session_id,
            subscription_id,
        } => {
            if conn.conn_peer.is_some() {
                return DaemonMessage::Error(
                    WireError::new(
                        ErrorCode::InvalidRequest,
                        "a peer cannot close a stream through this daemon",
                    )
                    .with_id(id),
                );
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
            }
        }
        ClientMessage::RemoteHostList {
            id,
            device_id,
            list,
        } => match state.peer_links.read(&device_id, list) {
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
        },
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
    )
}
