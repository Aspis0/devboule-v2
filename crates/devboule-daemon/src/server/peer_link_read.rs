//! One read over a live link: is the far end able to speak this frame at all,
//! write the request, and read back the frame that answers it.
//!
//! The link's worker owns the transport and calls in here. Nothing else touches
//! a socket for a read, and nothing here opens one: a read that cannot be
//! written is refused before a byte leaves.

use std::sync::Arc;
use std::time::{Duration, Instant};

use devboule_protocol::{caps, ClientMessage, DaemonMessage, RemoteHostList, RemoteHostState};

use super::peer_link::{
    needs_pairing_sentence, offline_sentence, sentence_for, state_for, unsupported_sentence,
};
use super::peer_link_state::{HostLink, LinkAnswer, LinkCommand};
use super::peer_link_worker::{peer_row, LinkSession, PROBE_ID_BASE};
use super::ServerState;
use crate::error::DaemonError;

/// Write one read and wait for its answer, answering the caller through the
/// channel it was queued with.
pub(crate) fn serve_read(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::Read {
        generation,
        list,
        answer,
    } = command
    else {
        return;
    };
    // The row is re-read before every request, not only at dial time: the link
    // may have been up for hours, and a revoke is a decision the user just
    // took rather than a transport fact that shows up on its own.
    if let Err(step) = peer_row(state, &link.device_id) {
        let _ = answer.send(LinkAnswer::Failed(state_for(step), sentence_for(step)));
        return;
    }
    // A read queued against the link that has just been replaced is refused, not
    // answered: its reply would belong to a transport this daemon no longer
    // has, and the generation it was queued with is the only thing that says so.
    if generation != link.generation() {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Offline,
            offline_sentence().to_string(),
        ));
        return;
    }
    if let Some(capability) = required_capability(&list) {
        if !session
            .hello
            .capabilities
            .iter()
            .any(|agreed| agreed.as_str() == capability)
        {
            let _ = answer.send(LinkAnswer::Failed(
                RemoteHostState::Unsupported,
                unsupported_sentence().to_string(),
            ));
            return;
        }
    }
    if session.framed.send(&list.peer_request(request_id)).is_err() {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Offline,
            offline_sentence().to_string(),
        ));
        return;
    }
    let _ = answer.send(wait_for_reply(
        link,
        session,
        &list,
        request_id,
        read_deadline,
    ));
}

/// Read frames until this read's id answers, a refusal arrives for it, or the
/// deadline passes.
///
/// A frame for another id is dropped rather than returned: after a reconnect
/// the remote is one link behind, and its old reply must not be handed to a
/// caller as this read's answer.
fn wait_for_reply(
    link: &HostLink,
    session: &mut LinkSession,
    list: &RemoteHostList,
    request_id: u64,
    read_deadline: Duration,
) -> LinkAnswer {
    let deadline = Instant::now() + read_deadline;
    while Instant::now() < deadline {
        // A revoke that lands mid-read stops the wait at once (P2-5).
        if link.is_revoked() {
            return LinkAnswer::Failed(
                RemoteHostState::NeedsPairing,
                needs_pairing_sentence().to_string(),
            );
        }
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(message) = session.framed.recv_timeout(left) else {
            // A closed socket after a revoke is still the revoke: check the
            // mark before classifying, so a real revoke is never reported
            // as Offline.
            if link.is_revoked() {
                return LinkAnswer::Failed(
                    RemoteHostState::NeedsPairing,
                    needs_pairing_sentence().to_string(),
                );
            }
            return LinkAnswer::Failed(RemoteHostState::Offline, offline_sentence().to_string());
        };
        if let DaemonMessage::Pong { id, .. } = &message {
            if *id >= PROBE_ID_BASE {
                session.outstanding_since = None;
                session.misses = 0;
                continue;
            }
        }
        // A push that lands mid-read is handled before the read's own answer
        // is matched; it names no request, so it must never be mistaken for
        // the reply.
        super::peer_link_worker::record_workspace_change(
            link,
            session.hello.protocol_version,
            &message,
        );
        forward_subscription(link, &message);
        if reply_id(&message) != Some(request_id) {
            continue;
        }
        if let DaemonMessage::Error(error) = message {
            // The remote's own refusal, reason intact: the app's empty state
            // says what the far side said, not what this daemon guessed.
            return LinkAnswer::Refused(error);
        }
        return match list.body_from_reply(&message) {
            Some(body) => LinkAnswer::Body(body),
            None => LinkAnswer::Failed(RemoteHostState::Offline, offline_sentence().to_string()),
        };
    }
    LinkAnswer::Failed(RemoteHostState::Offline, offline_sentence().to_string())
}

/// Relay one subscription event if the frame is one. `false` for every other
/// frame, so a caller can fall through to its own matching.
fn forward_subscription(link: &HostLink, message: &DaemonMessage) -> bool {
    let DaemonMessage::SubscriptionEvent {
        subscription_id,
        envelope,
    } = message
    else {
        return false;
    };
    link.forward_event(*subscription_id, envelope);
    true
}

/// Open one session's live stream on the far side and register the local
/// subscription the events come back to.
///
/// The subscription is registered before the request leaves, so a replayed
/// event that overtakes the attach reply is not lost; every refusal removes
/// it again. The far side's session scope and the `sessions` capability are
/// the trust floor: this daemon relays only what the peer's own `SessionAttach`
/// answered, and it names no session of its own.
pub(crate) fn serve_attach(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::Attach {
        generation,
        session_id,
        subscription_id,
        conn,
        answer,
    } = command
    else {
        return;
    };
    if let Err(step) = peer_row(state, &link.device_id) {
        let _ = answer.send(LinkAnswer::Failed(state_for(step), sentence_for(step)));
        return;
    }
    if generation != link.generation() {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Offline,
            offline_sentence().to_string(),
        ));
        return;
    }
    if !session
        .hello
        .capabilities
        .iter()
        .any(|agreed| agreed.as_str() == caps::SESSIONS)
    {
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Unsupported,
            unsupported_sentence().to_string(),
        ));
        return;
    }
    link.register_subscription(subscription_id, conn, session_id.clone());
    if session
        .framed
        .send(&ClientMessage::SessionAttach {
            id: request_id,
            session_id,
            subscription_id,
            from_cursor: None,
        })
        .is_err()
    {
        link.remove_subscription(subscription_id);
        let _ = answer.send(LinkAnswer::Failed(
            RemoteHostState::Offline,
            offline_sentence().to_string(),
        ));
        return;
    }
    let deadline = Instant::now() + read_deadline;
    while Instant::now() < deadline {
        // A revoke that lands mid-attach stops the wait at once (P2-5):
        // the subscription is already dropped below, so no event relays
        // after this either way.
        if link.is_revoked() {
            link.remove_subscription(subscription_id);
            let _ = answer.send(LinkAnswer::Failed(
                RemoteHostState::NeedsPairing,
                needs_pairing_sentence().to_string(),
            ));
            return;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        // Sliced like the operate wait so a mid-attach revoke is observed
        // promptly. Quiet slices loop on every platform's spelling of
        // silence; a dead transport breaks below — after checking the
        // revoke mark, so a real revoke is never reported as Offline.
        let slice = left.min(Duration::from_millis(50));
        let message = match session.framed.recv_timeout(slice) {
            Ok(message) => message,
            Err(DaemonError::TimedOut(_)) => continue,
            Err(DaemonError::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                continue
            }
            Err(_) => {
                if link.is_revoked() {
                    link.remove_subscription(subscription_id);
                    let _ = answer.send(LinkAnswer::Failed(
                        RemoteHostState::NeedsPairing,
                        needs_pairing_sentence().to_string(),
                    ));
                    return;
                }
                break;
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
        if forward_subscription(link, &message) {
            continue;
        }
        match message {
            DaemonMessage::SessionAttached { id, .. } if id == request_id => {
                let _ = answer.send(LinkAnswer::Accepted);
                return;
            }
            DaemonMessage::Error(error) if error.id == Some(request_id) => {
                link.remove_subscription(subscription_id);
                let _ = answer.send(LinkAnswer::Refused(error));
                return;
            }
            _ => {}
        }
    }
    link.remove_subscription(subscription_id);
    let _ = answer.send(LinkAnswer::Failed(
        RemoteHostState::Offline,
        offline_sentence().to_string(),
    ));
}

/// Close one session's live stream. The local subscription goes first, so no
/// event is relayed after the app asked to stop, and the peer's answer is
/// advisory: a link that is going away still leaves the local close standing.
pub(crate) fn serve_detach(
    link: &HostLink,
    session: &mut LinkSession,
    command: LinkCommand,
    request_id: u64,
    read_deadline: Duration,
) {
    let LinkCommand::Detach {
        generation,
        session_id,
        subscription_id,
        answer,
    } = command
    else {
        return;
    };
    link.remove_subscription(subscription_id);
    if generation != link.generation()
        || session
            .framed
            .send(&ClientMessage::SessionDetach {
                id: request_id,
                session_id,
                subscription_id,
            })
            .is_err()
    {
        let _ = answer.send(LinkAnswer::Accepted);
        return;
    }
    let deadline = Instant::now() + read_deadline;
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(message) = session.framed.recv_timeout(left) else {
            break;
        };
        if let DaemonMessage::Pong { id, .. } = &message {
            if *id >= PROBE_ID_BASE {
                session.outstanding_since = None;
                session.misses = 0;
                continue;
            }
        }
        forward_subscription(link, &message);
        if matches!(message, DaemonMessage::Ok { .. } | DaemonMessage::Error(..))
            && reply_id(&message) == Some(request_id)
        {
            let _ = answer.send(LinkAnswer::Accepted);
            return;
        }
    }
    let _ = answer.send(LinkAnswer::Accepted);
}

/// The request id a reply answers, or `None` for a frame that is not a reply.
fn reply_id(message: &DaemonMessage) -> Option<u64> {
    match message {
        DaemonMessage::Error(error) => error.id,
        DaemonMessage::Projects { id, .. }
        | DaemonMessage::Workspaces { id, .. }
        | DaemonMessage::Sessions { id, .. } => Some(*id),
        _ => None,
    }
}

/// The handshake capability the far daemon must advertise for this read.
///
/// A daemon that predates the frame cannot deserialize the request, so the
/// refusal happens here, before anything is written.
fn required_capability(list: &RemoteHostList) -> Option<&'static str> {
    match list {
        RemoteHostList::Projects | RemoteHostList::Workspaces { .. } => Some(caps::JOURNAL),
        RemoteHostList::Sessions => Some(caps::SESSIONS),
    }
}
