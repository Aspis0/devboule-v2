//! Rows from different generations sharing one replay.

use super::super::*;
use super::*;

use super::test_support::{drain, recovered_integrity};

/// Transcript rows from different generations can share a stream seq
/// (it restarts per generation); the recovered-transcript replay must
/// keep both and deliver them in (generation, seq) order.
#[test]
fn transcript_replay_keeps_rows_from_different_generations() {
    let integrity = recovered_integrity();
    let replay = crate::journal::Replay {
        generation: 2,
        last_seq: 1,
        integrity,
        event_seqs: vec![(1, 1), (1, 2), (2, 1), (2, 1)],
        event_ts_ms: vec![None; 4],
        events: vec![
            SessionEvent::AgentUserMessage {
                message_id: Some("m1".into()),
                text: "gen-1 user".into(),
                author: devboule_protocol::UserMessageAuthor::Human,
                message_kind: devboule_protocol::UserMessageKind::Unknown,
                at_ms: None,
                images: Vec::new(),
            },
            SessionEvent::AgentMessage {
                message_id: Some("m2".into()),
                text: "gen-1 answer".into(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
            SessionEvent::AgentMessage {
                message_id: Some("m3".into()),
                text: "after resume".into(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
            SessionEvent::Recovered { integrity },
        ],
    };
    let runtime = SessionRuntime::from_replay("s.cross.gen.view".to_string(), None, replay);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.cross.gen.view",
        Arc::clone(&runtime),
        true,
        Some(0),
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn);
    let transcript: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentUserMessage { text, .. } => Some(text.clone()),
            SessionEvent::AgentMessage { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        transcript,
        vec!["gen-1 user", "gen-1 answer", "after resume"],
        "rows from different generations must all survive the replay: {transcript:?}"
    );
}

/// Agent sessions journal Output rows (the permission-answered ledger)
/// per generation, so two generations can carry the same seq. The
/// transcript replay must not flatten them into one seq space: both
/// survive, ordered by (generation, seq).
#[test]
fn transcript_outputs_from_different_generations_do_not_collide() {
    let integrity = recovered_integrity();
    let replay = crate::journal::Replay {
        generation: 2,
        last_seq: 1,
        integrity,
        event_seqs: vec![(1, 1), (2, 1), (2, 1)],
        event_ts_ms: vec![None; 3],
        events: vec![
            SessionEvent::Output {
                seq: 1,
                data: "gen-1 ledger".to_string(),
            },
            SessionEvent::Output {
                seq: 1,
                data: "gen-2 ledger".to_string(),
            },
            SessionEvent::Recovered { integrity },
        ],
    };
    let runtime = SessionRuntime::from_replay("s.cross.gen.ledger".to_string(), None, replay);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.cross.gen.ledger",
        Arc::clone(&runtime),
        true,
        Some(0),
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn);
    let ledger: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { data, .. } => Some(data.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
            ledger,
            vec!["gen-1 ledger", "gen-2 ledger"],
            "same-seq outputs from different generations must both survive, in journal order: {ledger:?}"
        );
}
