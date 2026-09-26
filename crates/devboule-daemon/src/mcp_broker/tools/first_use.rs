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
    /// The gate mark for one session and group, if any. Test-only: production
    /// reads the gate through `ensure_write_allowed` alone.
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
/// Allowed opens the group for that session; denied, timed out or otherwise
/// unanswered refuses the call and leaves the gate shut, so the next call
/// asks again. A second call racing the first is refused as pending rather
/// than raising a second card.
pub(in crate::mcp_broker) fn ensure_write_allowed(
    state: &ServerState,
    broker: &McpBroker,
    session_id: &str,
    owner: &OwnerId,
    group: &str,
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
    let allowed = request_write_card(state, session_id, owner, group);
    let mut marks = broker
        .write_gates
        .marks
        .lock()
        .map_err(|_| "MCP state is unavailable.".to_string())?;
    let key = (session_id.to_string(), group.to_string());
    if allowed {
        marks.insert(key, GateMark::Open);
        Ok(())
    } else {
        marks.remove(&key);
        Err("permission refused".to_string())
    }
}

/// Raise the approval card through the same road the creation card uses and
/// wait for the person's answer.
fn request_write_card(state: &ServerState, session_id: &str, owner: &OwnerId, group: &str) -> bool {
    if state
        .sessions
        .live_runtime(session_id, owner)
        .and_then(|runtime| runtime.permission_broker())
        .is_none()
    {
        return false;
    }
    let card = write_gate_card(session_id, group);
    state.sessions.ask_creation_card(session_id, owner, card)
}

/// The approval card: an ordinary permission request with no creation
/// payload. The broker stamps the caller's own origin on the way in, so the
/// card carries the unknown placeholder here, as the creation card does.
fn write_gate_card(session_id: &str, group: &str) -> SessionEvent {
    SessionEvent::PermissionRequest {
        tool_call_id: write_gate_card_id(session_id, group),
        title: format!("Allow {group} changes for this session"),
        description: Some(format!(
            "An agent asked to change {group} for the first time. \
             Allowing approves {group} writes from this session from now on; \
             denying refuses this call."
        )),
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![
            PermissionOption {
                option_id: "allow".to_string(),
                name: "Allow".to_string(),
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
