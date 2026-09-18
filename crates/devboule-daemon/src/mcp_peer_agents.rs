//! The `devboule_list_peer_agents` tool: name one paired device, dial it
//! once, and render the live agent roster it answers with.
//!
//! One call names one device and makes one dial — never a fan-out, because
//! the daemon's whole outbound budget is four slots. The dial is bounded,
//! but not tightly: five seconds to connect, ten for the handshake, and a
//! ten-second deadline on each of the hello and request/reply legs — about
//! **fifty-five seconds worst case** — and the slot is held the whole time,
//! released only on drop. The pi bridge aborts the tool call at thirty
//! seconds, so the model can be told of a failure while the slot is still
//! held; the refusals below therefore never invite an immediate retry. The
//! broker answers each call on its own connection thread, so a slow or
//! asleep peer delays only the call that asked for it. Every failure is a
//! sentence a model can act on; a raw debug string never reaches the wire.

use std::sync::Arc;

use devboule_protocol::{ClientMessage, DaemonMessage, OwnerId};
use serde_json::{json, Value};

use crate::journal::PeerRecord;
use crate::server::{call_peer, DialError, ServerState};

/// The most roster entries one answer may carry past the boundary. The far
/// daemon owns every byte of its reply, so the count is bounded here, and
/// every drop is counted in the document the tool returns.
pub(crate) const MAX_PEER_ROSTER_ENTRIES: usize = 128;

/// The roster for one device, or the RPC error code plus the sentence the
/// broker answers with.
pub(crate) fn list_peer_agents(
    state: &Arc<ServerState>,
    caller: &OwnerId,
    device_id: &str,
) -> Result<Value, (i32, String)> {
    // The device is resolved from this daemon's rows by id first, then
    // attributed: absence, a pairing with no recorded user, and a pairing
    // that belongs to someone else are three different refusals.
    let row = state
        .peers()
        .map_err(|error| (-32603, error))?
        .into_iter()
        .find(|record| record.device_id == device_id);
    let Some(row) = row else {
        return Err((
            -32602,
            format!(
                "No paired device named '{device_id}' is paired by this session's user; use \
                 devboule_list_devices to name a device this session can call."
            ),
        ));
    };
    // `paired_by_user` is always `None` on a platform without user ids and
    // on pairings that predate the recording, so this session cannot tell
    // whether the device is its to call. That is its own refusal — not the
    // absent one, and never a silent pass.
    let Some(paired_user) = &row.paired_by_user else {
        return Err((
            -32602,
            format!(
                "The pairing with {} has no user recorded on this machine, so this session \
                 cannot tell whether it is the device's pair; re-pair to record the user.",
                row.display_name
            ),
        ));
    };
    if paired_user != &caller.user {
        return Err((
            -32602,
            format!(
                "No paired device named '{device_id}' is paired by this session's user; use \
                 devboule_list_devices to name a device this session can call."
            ),
        ));
    }
    // A revoked row is a different fact from an absent one: the pairing is
    // gone, and only re-pairing brings it back.
    if row.revoked_at.is_some() {
        return Err((
            -32602,
            format!(
                "The pairing with {} is gone; pair it again before calling it.",
                row.display_name
            ),
        ));
    }
    match call_peer(state, device_id, ClientMessage::PeerAgentsList { id: 0 }) {
        Ok(DaemonMessage::PeerAgents { agents, scope, .. }) => {
            // The responder's own "cannot scope" verdict: its pairing row
            // recorded no user, so the roster it would expose answers to
            // nobody in particular. Not an empty roster, and not this
            // session's data to read.
            if scope == devboule_protocol::PeerRosterScope::Unscoped {
                return Err((
                    -32602,
                    format!(
                        "{} cannot scope its roster: its pairing recorded no user, so it \
                         answers for nobody; re-pair it to record the user.",
                        row.display_name
                    ),
                ));
            }
            let (agents, dropped) = boundary_pass(agents);
            Ok(roster_document(&row, agents, dropped))
        }
        Ok(_) => Err((
            -32602,
            format!(
                "{} answered the roster request with an unexpected message; try again later.",
                row.display_name
            ),
        )),
        Err(error) => Err((-32602, dial_error_sentence(&error, &row))),
    }
}

/// The boundary every far-daemon roster passes through, once, on the way in —
/// before the renderer, the JSON envelope, or anything that might reach a
/// model. The far daemon owns every byte of its reply, so nothing is trusted:
/// control characters and bidi overrides go (the same set
/// `provider_catalog::strip_control_and_bidi` strips from version labels),
/// then the permission-card path's own two passes —
/// `session::neutralise_envelope_text` for frame-structure delimiters,
/// `session::single_line_header` for the single-line shape and the per-field
/// cap. Entries past [`MAX_PEER_ROSTER_ENTRIES`] are dropped, and the count
/// rides the document: a silently shortened roster would be a lie about the
/// far machine.
fn boundary_pass(
    agents: Vec<devboule_protocol::PeerAgent>,
) -> (Vec<devboule_protocol::PeerAgent>, usize) {
    let dropped = agents.len().saturating_sub(MAX_PEER_ROSTER_ENTRIES);
    let kept = agents
        .into_iter()
        .take(MAX_PEER_ROSTER_ENTRIES)
        .map(|agent| devboule_protocol::PeerAgent {
            session_id: far_text(&agent.session_id),
            name: far_text(&agent.name),
            provider: agent.provider.as_deref().map(far_text),
            model: agent.model.as_deref().map(far_text),
            state: agent.state,
            depth: agent.depth,
        })
        .collect();
    (kept, dropped)
}

/// One far-daemon string, made safe to carry: the three existing passes
/// composed, no new neutraliser.
fn far_text(value: &str) -> String {
    crate::session::single_line_header(&crate::session::neutralise_envelope_text(
        &crate::provider_catalog::strip_control_and_bidi(value),
    ))
}

/// The document one successful roster call renders: the device named by id
/// and display name, its agents as the responder answered them (after the
/// boundary pass), and `truncated` — how many entries the boundary dropped,
/// present even when zero, so a shortened roster is never silent. The pair
/// (deviceId, sessionId) identifies each entry — a session id is unique only
/// within one daemon — and `agents` is what is live now, never a stored
/// object.
fn roster_document(
    row: &PeerRecord,
    agents: Vec<devboule_protocol::PeerAgent>,
    truncated: usize,
) -> Value {
    json!({
        "deviceId": row.device_id,
        "deviceName": row.display_name,
        "agents": agents,
        "truncated": truncated,
    })
}

/// The sentence for a dial that failed, mapped from the step it failed at:
/// what happened and what to do about it, never the debug string.
fn dial_error_sentence(error: &DialError, row: &PeerRecord) -> String {
    let name = &row.display_name;
    match error.step() {
        "row_missing" => format!(
            "No paired device named '{name}' is paired by this session's user; use \
             devboule_list_devices to name a device this session can call."
        ),
        "revoked" => format!(
            "The pairing with {name} was revoked while the call was in flight; pair it again."
        ),
        // "Try again shortly" would invite the retry loop that saturates the
        // four slots — one call to a slow device holds its slot for most of
        // a minute.
        "busy" => "This daemon is already using all of its outbound call slots, which one call \
                   to a slow device can hold for most of a minute. Wait for other calls to \
                   finish before calling again."
            .to_string(),
        "no_listen_port" => format!(
            "{name} never advertised a listener, so there is nothing to dial: re-pair it if it \
             is a daemon; a client device cannot be dialled."
        ),
        "address" => format!("The stored address for {name} is not dialable; re-pair the device."),
        "identity" => {
            "This device's own identity is unavailable, so it cannot make the call.".to_string()
        }
        "unsupported" => format!(
            "{name} runs a daemon that predates the agent roster, so it cannot answer this \
             call; update the far daemon first."
        ),
        // connect, handshake, hello, send, reply, unsupported: the far end
        // did not complete the call. The step name is the stable token, not
        // a dump. The remedy names the device's state, not a retry time: a
        // machine that is asleep stays asleep however soon the call repeats.
        step => format!(
            "{name} did not complete the call ({step}); it may be asleep or offline. Check the \
             device is running before calling it again."
        ),
    }
}

#[cfg(test)]
#[path = "mcp_peer_agents_tests.rs"]
mod tests;
