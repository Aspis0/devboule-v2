//! Seed a live Claude checklist from the session's journal, and the byte
//! gates that keep the fold and the live trigger from parsing envelopes that
//! cannot move it.

use super::SessionRuntime;
use crate::claude_view::ClaudeView;

/// Seed the live checklist from the journalled envelopes, folding page by
/// page through the shared replay driver. After a restart the app holds the
/// replayed list, and without the seed the first live `TaskUpdate` would find
/// no task and the pill would freeze.
///
/// The fold holds no envelopes: each page is parsed, driven and dropped, and
/// only envelopes that can move the checklist parse at all — task tools by
/// name, result blocks while a call is pending. The seed runs only once a
/// live line names a task tool, so a task-less session pays nothing here.
///
/// Returns whether the seed completed: the caller flags success only then,
/// so a failed seed retries on the next frame instead of freezing silently.
pub(super) fn seed_claude_task_state(runtime: &SessionRuntime, view: &mut ClaudeView) -> bool {
    let generation = runtime.generation();
    let mut seed_view = ClaudeView::new(None);
    let (mut from_generation, mut from_seq) = (0, 0);
    loop {
        let page = match runtime.replay_journal_agent_page(
            generation,
            from_generation,
            from_seq,
            u64::MAX,
            512,
        ) {
            Ok(Some(page)) => page,
            Ok(None) => return true,
            Err(error) => {
                runtime.note_task_seed_failure();
                eprintln!("task seed failed: {error}");
                return false;
            }
        };
        if page.records.is_empty() {
            break;
        }
        for record in &page.records {
            // Three cases: task names open pairs, result blocks parse while
            // a call is pending, and lifecycle frames always parse — init
            // rebinds the list, turn end clears pending calls, one frame
            // each. Turn end drops pending calls, so the result window stays
            // shut for task-less stretches.
            if record.kind == crate::journal::EventKind::AcpEnvelope
                && (envelope_may_carry_tasks(&record.payload)
                    || envelope_is_lifecycle(&record.payload)
                    || (seed_view.has_pending_task_calls()
                        && envelope_is_tool_result(&record.payload)))
            {
                if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&record.payload)
                {
                    let _ = crate::claude_view::drive_replay(&mut seed_view, &mut value);
                }
            }
            from_generation = record.generation;
            from_seq = record.seq;
        }
        if page.records.len() < 512 {
            break;
        }
    }
    view.restore_task_state(seed_view.snapshot_task_state());
    true
}

/// Byte-level gate before JSON parsing: only envelopes naming a task tool
/// can open a pair. Results name no tool — the seed parses those while a
/// call is pending (see below) — so a session that never issued one skips
/// every parse.
///
/// A performance filter, not a correctness filter: a false positive only
/// parses an envelope `observe` then ignores, and the quoted names assume
/// `serde_json`-written payloads (the only writer of these rows).
pub(super) fn envelope_may_carry_tasks(payload: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(payload) else {
        return true;
    };
    text.contains("\"TodoWrite\"")
        || text.contains("\"TaskCreate\"")
        || text.contains("\"TaskUpdate\"")
        || text.contains("\"TaskList\"")
}

/// Whether the payload carries a tool result block: the other half of a
/// task-tool pair, parsed only while `has_pending_calls`.
///
/// Same performance-filter contract as `envelope_may_carry_tasks`.
fn envelope_is_tool_result(payload: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(payload) else {
        return true;
    };
    text.contains("\"tool_result\"")
}

/// Lifecycle frames the checklist machine needs even when they name no
/// task tool: `system/init` (a new CLI session id rebinds — the list starts
/// empty; Paseo's `handleSystemMessage` reads `session_id` off init,
/// `agent.test.ts:1966`) and turn-end `result` (pending calls clear there).
/// One per session and one per turn, so admitting them costs nothing next
/// to the pairs.
///
/// Same performance-filter contract as `envelope_may_carry_tasks`.
fn envelope_is_lifecycle(payload: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(payload) else {
        return true;
    };
    text.contains("\"subtype\":\"init\"") || text.contains("\"type\":\"result\"")
}

#[cfg(test)]
#[path = "claude_task_seed_tests.rs"]
mod tests;
