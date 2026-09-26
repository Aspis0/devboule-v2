//! The first-use human card for write tools, shared by group name.
//!
//! One responsibility: remembering which sessions a human already approved
//! for a write group, and raising the approval card once each.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use devboule_protocol::{OwnerId, PermissionOption, SessionEvent, SessionOrigin};

use crate::mcp_broker::McpBroker;
use crate::server::ServerState;

/// The write group C3a gates. C2b adds `"terminals"` here and C3b reuses
/// this one; the group travels as a plain name so later slices only call.
pub(in crate::mcp_broker) const WORKSPACES_GROUP: &str = "workspaces";

/// The card choice that approves only the call it was raised for.
const CHOICE_ONCE: &str = "once";
/// The card choice that approves the group for the calling session.
const CHOICE_SESSION: &str = "session";

/// Whether a session may call a write group without asking again.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::mcp_broker) enum GateMark {
    Pending,
    Open,
}

#[derive(Default)]
pub(in crate::mcp_broker) struct FirstUseGates {
    marks: Mutex<HashMap<(String, String), GateMark>>,
}

impl McpBroker {
    /// The gate mark for one session and group, if any. Test-only:
    /// production reads the gate through `ensure_write_allowed` alone.
    #[cfg(test)]
    pub(in crate::mcp_broker) fn first_use_mark(
        &self,
        session_id: &str,
        group: &str,
    ) -> Option<GateMark> {
        self.write_gates
            .marks
            .lock()
            .ok()?
            .get(&(session_id.to_string(), group.to_string()))
            .copied()
    }

    /// Forget every mark of one session: called when its bearer goes away,
    /// so a later session never inherits an approval it did not ask for.
    pub(in crate::mcp_broker) fn forget_first_use(&self, session_id: &str) {
        if let Ok(mut marks) = self.write_gates.marks.lock() {
            marks.retain(|(session, _), _| session != session_id);
        }
    }
}

/// Pass the write gate for `group`, raising the human card on the caller's
/// own session the first time.
///
/// `subject` says what this call is about ("creating workspace 'Desk'") and
/// `facts` are the `key: value` lines the card shows under it. Both travel
/// on the card because a licence without them asks the person to approve
/// what they cannot see.
///
/// The card is a chooser — two allow options of one kind — so the app shows
/// both choices by name and the delegation door refuses it the way it
/// refuses every question: only a person answers. "Allow this call"
/// proceeds without opening the group; "Allow for this session" opens it.
/// Denied, timed out or otherwise unanswered refuses the call and leaves
/// the gate shut, so the next call asks again. A second call racing the
/// first is refused as pending rather than raising a second card.
pub(in crate::mcp_broker) fn ensure_write_allowed(
    state: &ServerState,
    broker: &McpBroker,
    session_id: &str,
    owner: &OwnerId,
    group: &str,
    subject: &str,
    facts: &[(&str, &str)],
) -> Result<(), String> {
    {
        let mut marks = broker
            .write_gates
            .marks
            .lock()
            .map_err(|_| "MCP state is unavailable.".to_string())?;
        match marks.get(&(session_id.to_string(), group.to_string())) {
            Some(GateMark::Open) => return Ok(()),
            Some(GateMark::Pending) => {
                return Err("permission pending; retry".to_string());
            }
            None => {
                marks.insert(
                    (session_id.to_string(), group.to_string()),
                    GateMark::Pending,
                );
            }
        }
    }
    let card = write_gate_card(session_id, group, subject, facts);
    let card_id = gate_card_id(&card).to_string();
    let allowed = request_write_card(state, session_id, owner, &card_id, card);
    let mut marks = broker
        .write_gates
        .marks
        .lock()
        .map_err(|_| "MCP state is unavailable.".to_string())?;
    let key = (session_id.to_string(), group.to_string());
    if allowed == Some(CHOICE_SESSION) {
        marks.insert(key, GateMark::Open);
        Ok(())
    } else if allowed == Some(CHOICE_ONCE) {
        marks.remove(&key);
        Ok(())
    } else {
        marks.remove(&key);
        Err("permission refused".to_string())
    }
}

/// Raise the approval card through the same road the creation card uses and
/// wait for the person's answer. Answers the option id the person chose, or
/// `None` when the card was never raised or the answer was not an allow.
fn request_write_card(
    state: &ServerState,
    session_id: &str,
    owner: &OwnerId,
    card_id: &str,
    card: SessionEvent,
) -> Option<&'static str> {
    let runtime = state.sessions.live_runtime(session_id, owner)?;
    let card_broker = runtime.permission_broker()?;
    card_broker.watch_card_choice(card_id);
    let raised = state.sessions.ask_creation_card(session_id, owner, card);
    let choice = card_broker.take_card_choice(card_id).flatten();
    if !raised {
        return None;
    }
    choice_as_static(choice)
}

/// The watched answer as the gate reads it. Kept beside the take so the
/// two spellings of each choice cannot drift.
fn choice_as_static(choice: Option<String>) -> Option<&'static str> {
    match choice.as_deref() {
        Some(CHOICE_ONCE) => Some(CHOICE_ONCE),
        Some(CHOICE_SESSION) => Some(CHOICE_SESSION),
        _ => None,
    }
}

/// The approval card: an ordinary permission request with no creation
/// payload. The broker stamps the caller's own origin on the way in, so the
/// card carries the unknown placeholder here, as the creation card does.
///
/// Two allow options of one kind make this a chooser by Paseo's rule (the
/// same kind twice is a question): the app renders both choices by name,
/// and no agent — delegated, peer or auto-answered — can pick one. The
/// journal's grant kind is `allow_once` for both, as for every answered
/// chooser; which choice was made rides the resolved event's option id.
fn write_gate_card(
    session_id: &str,
    group: &str,
    subject: &str,
    facts: &[(&str, &str)],
) -> SessionEvent {
    let listed = facts
        .iter()
        .map(|(key, value)| format!("{key}: {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let description = if listed.is_empty() {
        format!(
            "An agent asked to {subject} for the first time. \
             \"Allow this call\" approves only this call. \
             \"Allow {group} for this session\" approves {group} writes from this session from now on."
        )
    } else {
        format!(
            "An agent asked to {subject} for the first time:\n{listed}\n\n\
             \"Allow this call\" approves only this call. \
             \"Allow {group} for this session\" approves {group} writes from this session from now on."
        )
    };
    SessionEvent::PermissionRequest {
        tool_call_id: write_gate_card_id(session_id, group),
        title: format!("Allow {subject}"),
        description: Some(description),
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![
            PermissionOption {
                option_id: CHOICE_ONCE.to_string(),
                name: "Allow this call".to_string(),
                kind: "allow_once".to_string(),
            },
            PermissionOption {
                option_id: CHOICE_SESSION.to_string(),
                name: format!("Allow {group} for this session"),
                kind: "allow_once".to_string(),
            },
            PermissionOption {
                option_id: "deny".to_string(),
                name: "Deny".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
        is_chooser: None,
        origin: SessionOrigin::unknown(),
        create_agent: None,
    }
}

/// The card id of a gate card just built, for the watch the gate sets
/// before raising it.
fn gate_card_id(card: &SessionEvent) -> &str {
    match card {
        SessionEvent::PermissionRequest { tool_call_id, .. } => tool_call_id,
        _ => "",
    }
}

/// The correlation id of one gate card. Distinct per call, so two sessions
/// racing the same group cannot collide in the pending table.
fn write_gate_card_id(session_id: &str, group: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "write:{group}:{session_id}:{:x}-{}",
        nanos,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
#[path = "mcp_first_use_tests.rs"]
mod tests;
