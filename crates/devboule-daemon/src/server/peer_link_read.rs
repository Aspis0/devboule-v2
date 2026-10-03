//! One read over a live link: is the far end able to speak this frame at all,
//! write the request, and read back the frame that answers it.
//!
//! The link's worker owns the transport and calls in here. Nothing else touches
//! a socket for a read, and nothing here opens one: a read that cannot be
//! written is refused before a byte leaves.

use std::sync::Arc;
use std::time::{Duration, Instant};

use devboule_protocol::{caps, DaemonMessage, RemoteHostList, RemoteHostState};

use super::peer_link::{offline_sentence, sentence_for, state_for, unsupported_sentence};
use super::peer_link_state::{HostLink, LinkAnswer, LinkCommand};
use super::peer_link_worker::{peer_row, LinkSession, PROBE_ID_BASE};
use super::ServerState;

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
    } = command;
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
    let _ = answer.send(wait_for_reply(session, &list, request_id, read_deadline));
}

/// Read frames until this read's id answers, a refusal arrives for it, or the
/// deadline passes.
///
/// A frame for another id is dropped rather than returned: after a reconnect
/// the remote is one link behind, and its old reply must not be handed to a
/// caller as this read's answer.
fn wait_for_reply(
    session: &mut LinkSession,
    list: &RemoteHostList,
    request_id: u64,
    read_deadline: Duration,
) -> LinkAnswer {
    let deadline = Instant::now() + read_deadline;
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(message) = session.framed.recv_timeout(left) else {
            return LinkAnswer::Failed(RemoteHostState::Offline, offline_sentence().to_string());
        };
        if let DaemonMessage::Pong { id, .. } = &message {
            if *id >= PROBE_ID_BASE {
                session.outstanding_since = None;
                session.misses = 0;
                continue;
            }
        }
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
