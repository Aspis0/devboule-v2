//! Recover a Codex turn's plan mode from journalled rows.
//!
//! The live reader tracks the mode per turn from the mode the turn was sent
//! with, but outbound `turn/start` frames are not journalled. What the
//! journal does keep is the turn's approval trace: plan-mode-ON turns that
//! complete cleanly raise a plan card under the `{turn}-plan` id, and every
//! resolution journals the card's verdict row beside it. Both replay paths
//! read these marks up front through this module, so they learn the mode
//! identically.

use std::collections::HashSet;

use devboule_protocol::SessionEvent;
use rusqlite::Connection;

use crate::journal::JournalError;

/// Whether a raw `agent_report` payload can mark a plan turn: byte-level gate
/// before JSON parsing, so the scan skips every payload that cannot carry a
/// `{turn}-plan` id without deserialising it.
///
/// A performance filter, not a correctness filter: a false positive only
/// parses a payload `plan_card_turn_id` then rejects, and the substring
/// assumes `serde_json`-written payloads (the only writer of these rows).
pub(crate) fn payload_may_mark(payload: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(payload) else {
        return true;
    };
    text.contains("-plan")
}

/// The turn a journalled plan mark belongs to: `PermissionRequest {
/// kind: Plan }` under the `{turn}-plan` card id (exact), or the journalled
/// verdict row (`AgentToolUpdate { kind: \"plan\" }` with the card's verdict
/// as its title — the request itself rides live only, the verdict is what
/// the journal keeps). Plan item rows never match: their title is always the
/// plain `\"Plan\"`.
pub(crate) fn plan_card_turn_id(event: &SessionEvent) -> Option<&str> {
    match event {
        SessionEvent::PermissionRequest {
            tool_call_id,
            kind: Some(devboule_protocol::PermissionRequestKind::Plan),
            ..
        } => tool_call_id
            .strip_suffix("-plan")
            .filter(|turn| !turn.is_empty()),
        SessionEvent::AgentToolUpdate {
            tool_call_id,
            title: Some(title),
            kind: Some(kind),
            ..
        } if kind == "plan" && title != "Plan" => tool_call_id
            .strip_suffix("-plan")
            .filter(|turn| !turn.is_empty()),
        _ => None,
    }
}

/// Turns with a plan mark anywhere in the session's journal, all generations:
/// turn ids are unique across the thread's life, so one pass serves every
/// replay of the session.
pub(crate) fn scan_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<HashSet<String>, JournalError> {
    let mut stmt =
        conn.prepare("SELECT payload FROM events WHERE session_id = ?1 AND kind = 'agent_report'")?;
    let rows = stmt.query_map([session_id], |row| row.get::<_, Vec<u8>>(0))?;
    let mut turns = HashSet::new();
    for row in rows {
        let payload = row?;
        if !payload_may_mark(&payload) {
            continue;
        }
        if let Ok(event) = serde_json::from_slice::<SessionEvent>(&payload) {
            if let Some(turn) = plan_card_turn_id(&event) {
                turns.insert(turn.to_string());
            }
        }
    }
    Ok(turns)
}
