//! Wire-only resolution for a daemon restart's orphaned permission cards.
//!
//! The broker's pending table is memory only: a restart takes the answerer
//! with it, and the session a client then opens is a hydrated transcript
//! with no broker at all. A request the journal still shows unresolved
//! replays as a card nobody can ever answer, and every answer fails with
//! "Session has no live ACP permission broker".
//!
//! Hydration appends one synthetic `PermissionResolved` per unresolved
//! request to the in-memory replay alone — never journaled, no ledger row,
//! no `PermissionAnswered`, because nobody answered. History keeps no
//! verdict it never made: a card resolved before 2026-09-14 has a ledger
//! row and a `PermissionResolved` but no attribution event, and writing one
//! with a fresh outcome would falsify that row. A read-only journal still
//! hydrates, and the pass cannot fail, so a transcript never becomes
//! unopenable.

use std::collections::HashSet;

use devboule_protocol::SessionEvent;

use crate::journal::Replay;

/// One synthetic `PermissionResolved` for every request in `replay.events`
/// with no later `PermissionResolved` for the same `tool_call_id`, appended
/// after the last event. A transcript already resolved — `PermissionResolved`
/// present, of any era, with or without attribution or ledger row — gets
/// nothing.
///
/// Pure computation over the replayed events: no journal call, so the caller
/// needs no error path and no lock. Each synthetic is carried as an ordinary
/// `event_seqs` entry past `last_seq`, because `from_replay` drops rows with
/// no seq and the pull serves map rows in key order after the request; with
/// `last_seq` advanced past the tail the runtime's `next_seq` starts past
/// the synthetics, the way journaled rows maintain it. The journal's own
/// `last_seq` is never written, so every hydration recomputes the same tail
/// and the journal row count never moves.
pub(super) fn resolve_orphans(replay: &mut Replay) {
    let (requests, resolved) = scan(&replay.events);
    let mut added = 0u64;
    let mut settled: HashSet<String> = HashSet::new();
    for tool_call_id in &requests {
        if resolved.contains(tool_call_id) || !settled.insert(tool_call_id.clone()) {
            continue;
        }
        replay.events.push(SessionEvent::PermissionResolved {
            tool_call_id: tool_call_id.clone(),
            selected_option_id: None,
            selected_option_kind: None,
            selected_option_name: None,
            answered_by: None,
        });
        replay
            .event_seqs
            .push((replay.generation, replay.last_seq.saturating_add(added + 1)));
        added += 1;
    }
    if added > 0 {
        // The tail keys sit under `last_seq`, never in the journal: a later
        // hydration re-reads the same rows and rebuilds the same tail.
        replay.last_seq = replay.last_seq.saturating_add(added);
    }
}

/// The request cards in journal order, and the ids a `PermissionResolved`
/// already covers — of any era, with or without attribution or ledger row.
/// `PermissionAnswered` is deliberately not consulted: pre-2026-09-14
/// resolutions have none, and their ledger row is the verdict already made.
fn scan(events: &[SessionEvent]) -> (Vec<String>, HashSet<String>) {
    let mut requests: Vec<String> = Vec::new();
    let mut resolved: HashSet<String> = HashSet::new();
    for event in events {
        match event {
            SessionEvent::PermissionRequest { tool_call_id, .. } => {
                requests.push(tool_call_id.clone());
            }
            SessionEvent::PermissionResolved { tool_call_id, .. } => {
                resolved.insert(tool_call_id.clone());
            }
            _ => {}
        }
    }
    (requests, resolved)
}
