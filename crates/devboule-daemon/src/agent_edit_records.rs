//! The agent edit records of one workspace, read from the journal.
//!
//! A session's tool calls come back from the journal replay as events, and the
//! edits a call carried ride inside its event. This read replays each session of
//! the workspace, closed ones included, and keeps the calls that carried edits:
//! the last record of each call wins, since a call is recorded again as its
//! status moves.
//!
//! Every bound is reported, never silent: sessions past the cap and transcripts
//! past the byte cap set `truncated`, and a session whose transcript will not
//! read is a gap the answer says it has, not an empty answer.

use std::collections::BTreeMap;

use devboule_protocol::{AgentEditText, AgentToolEdit, SessionEvent, SessionKind};

use crate::journal::Journal;

/// Edits older than this are not listed: the Changes panel answers "what did
/// agents just change here", not "what ever happened".
pub(crate) const AGENT_EDIT_WINDOW_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// Most sessions one read replays, newest first.
pub(crate) const MAX_READ_SESSIONS: usize = 32;

/// The largest transcript (stored payload bytes) one read replays into
/// memory. A bigger one is skipped and the answer says so.
pub(crate) const MAX_SESSION_BYTES: u64 = 32 * 1024 * 1024;

/// One file change one tool call recorded, with the session that made it.
pub(crate) struct AgentEditRecord {
    pub session_id: String,
    /// The session's display name, falling back to its title.
    pub agent: String,
    pub tool_call_id: String,
    /// When the call's newest record was written, in epoch milliseconds.
    pub at_ms: u64,
    pub path: String,
    pub text: Option<AgentEditText>,
    pub added: u64,
    pub removed: u64,
    pub truncated: bool,
}

/// What one workspace read found, oldest first.
pub(crate) struct WorkspaceAgentEditRecords {
    pub records: Vec<AgentEditRecord>,
    /// A bound dropped records: a session past the cap of sessions, over the
    /// byte cap, or unreadable.
    pub truncated: bool,
    /// The read could not run at all. The sentence is static: it names no path.
    pub error: Option<String>,
}

pub(crate) fn read(
    journal: &Journal,
    workspace_id: &str,
    now_ms: u64,
) -> WorkspaceAgentEditRecords {
    let Ok(reader) = journal.open_reader() else {
        return failed();
    };
    let Ok(sessions) = crate::journal::sessions_of_workspace_on(&reader, workspace_id) else {
        return failed();
    };
    let window_start = now_ms.saturating_sub(AGENT_EDIT_WINDOW_MS);
    let mut truncated = sessions.len() > MAX_READ_SESSIONS;
    let mut records = Vec::new();
    for session in sessions.iter().take(MAX_READ_SESSIONS) {
        if session.kind == SessionKind::Terminal {
            continue;
        }
        if session.payload_bytes > MAX_SESSION_BYTES {
            truncated = true;
            continue;
        }
        let Ok(replay) = crate::journal::replay_session_including_closed_on(&reader, &session.id)
        else {
            truncated = true;
            continue;
        };
        let agent = session
            .display_name
            .clone()
            .unwrap_or_else(|| session.title.clone());
        records.extend(session_records(
            &session.id,
            &agent,
            session.updated_at_ms,
            &replay.events,
            &replay.event_ts_ms,
        ));
    }
    records.retain(|record| record.at_ms >= window_start);
    records.sort_by(|left, right| {
        left.at_ms
            .cmp(&right.at_ms)
            .then_with(|| left.session_id.cmp(&right.session_id))
            .then_with(|| left.tool_call_id.cmp(&right.tool_call_id))
            .then_with(|| left.path.cmp(&right.path))
    });
    WorkspaceAgentEditRecords {
        records,
        truncated,
        error: None,
    }
}

/// One tool call as its replay ends: the edits it carried, the status it
/// finished with, and when that was last recorded.
struct CallState<'a> {
    at_ms: u64,
    status: &'a str,
    edits: Option<&'a [AgentToolEdit]>,
}

/// The edit records of one session's replay, one per file change of each call
/// that finished `completed`. A denied, failed or unfinished call changed
/// nothing the person can rely on, so it is not listed.
/// `event_ts_ms` is index-aligned with `events`; an event without a time is
/// stamped with `fallback_ms`, the session's last update.
fn session_records(
    session_id: &str,
    agent: &str,
    fallback_ms: u64,
    events: &[SessionEvent],
    event_ts_ms: &[Option<u64>],
) -> Vec<AgentEditRecord> {
    let mut calls: BTreeMap<&str, CallState> = BTreeMap::new();
    for (index, event) in events.iter().enumerate() {
        let at_ms = event_ts_ms
            .get(index)
            .copied()
            .flatten()
            .unwrap_or(fallback_ms);
        match event {
            SessionEvent::AgentToolCall {
                tool_call_id,
                status,
                edits,
                ..
            } => {
                let call = calls.entry(tool_call_id.as_str()).or_insert(CallState {
                    at_ms,
                    status: status.as_str(),
                    edits: None,
                });
                call.at_ms = at_ms;
                call.status = status.as_str();
                if let Some(edits) = edits {
                    call.edits = Some(edits.as_slice());
                }
            }
            SessionEvent::AgentToolUpdate {
                tool_call_id,
                status: Some(status),
                ..
            } => {
                if let Some(call) = calls.get_mut(tool_call_id.as_str()) {
                    call.at_ms = at_ms;
                    call.status = status.as_str();
                }
            }
            _ => {}
        }
    }
    calls
        .into_iter()
        .filter(|(_, call)| call.status == "completed")
        .filter_map(|(tool_call_id, call)| {
            call.edits.map(|edits| (tool_call_id, call.at_ms, edits))
        })
        .flat_map(|(tool_call_id, at_ms, edits)| {
            edits.iter().map(move |edit| AgentEditRecord {
                session_id: session_id.to_string(),
                agent: agent.to_string(),
                tool_call_id: tool_call_id.to_string(),
                at_ms,
                path: edit.path.clone(),
                text: edit.text.clone(),
                added: edit.added,
                removed: edit.removed,
                truncated: edit.truncated,
            })
        })
        .collect()
}

fn failed() -> WorkspaceAgentEditRecords {
    WorkspaceAgentEditRecords {
        records: Vec::new(),
        truncated: false,
        error: Some("the conversation journal could not be read".to_string()),
    }
}

#[cfg(test)]
#[path = "agent_edit_records_tests.rs"]
mod tests;
