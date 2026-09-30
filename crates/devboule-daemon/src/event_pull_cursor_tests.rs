//! The transcript cursor: what advances it and what replays after it.

use super::super::*;
use super::*;

use super::test_support::{drain, recovered_integrity};

#[test]
fn transcript_cursor_replays_only_after() {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.bump_generation();
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(
            Some(Cursor {
                generation: 2,
                seq: 1,
            }),
            &conn,
            false,
        )
        .unwrap();
    conn.track_with_agent_replay(
        "s.a.1",
        Arc::clone(&runtime),
        true,
        Some(1),
        outcome.generation,
        outcome.live_agent_replay,
    );
    runtime.stream.lock().unwrap().scrollback.push(2, 2, b"two");
    runtime.finish(Some(0));
    let events = drain(&conn);
    assert_eq!(
        events,
        vec![
            SessionEvent::Output {
                seq: 2,
                data: "two".to_string()
            },
            SessionEvent::Exit { code: Some(0) },
        ]
    );
}

#[test]
fn observers_replay_from_independent_cursors() {
    let integrity = recovered_integrity();
    let replay = crate::journal::Replay {
        generation: 1,
        last_seq: 3,
        integrity,
        event_seqs: vec![(1, 1), (1, 2), (1, 3), (1, 3)],
        event_ts_ms: vec![None; 4],
        events: vec![
            SessionEvent::Output {
                seq: 1,
                data: "one".to_string(),
            },
            SessionEvent::Output {
                seq: 2,
                data: "two".to_string(),
            },
            SessionEvent::Output {
                seq: 3,
                data: "three".to_string(),
            },
            SessionEvent::Recovered { integrity },
        ],
    };
    let runtime = Arc::new(SessionRuntime::from_replay(
        "s.independent-cursors".to_string(),
        None,
        replay,
    ));
    let first = ConnHandle::new(11);
    let second = ConnHandle::new(22);
    let first_outcome = runtime
        .try_attach_with_subscription(
            1101,
            Some(Cursor {
                generation: 1,
                seq: 0,
            }),
            &first,
            false,
        )
        .expect("first replay observer");
    first
        .track_with_subscription(
            1101,
            Arc::clone(&runtime),
            true,
            Some(0),
            first_outcome.generation,
            first_outcome.live_agent_replay,
        )
        .expect("first subscription");
    let second_outcome = runtime
        .try_attach_with_subscription(
            2202,
            Some(Cursor {
                generation: 1,
                seq: 1,
            }),
            &second,
            false,
        )
        .expect("second replay observer");
    second
        .track_with_subscription(
            2202,
            Arc::clone(&runtime),
            true,
            Some(1),
            second_outcome.generation,
            second_outcome.live_agent_replay,
        )
        .expect("second subscription");

    let output_text = |events: Vec<SessionEvent>| {
        events
            .into_iter()
            .filter_map(|event| match event {
                SessionEvent::Output { data, .. } => Some(data),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(output_text(drain(&first)), vec!["one", "two", "three"]);
    assert_eq!(output_text(drain(&second)), vec!["two", "three"]);
}

#[test]
fn transcript_cursor_advances_past_agent_reported() {
    let integrity = recovered_integrity();
    let replay = crate::journal::Replay {
        generation: 1,
        last_seq: 3,
        integrity,
        event_seqs: vec![(1, 2), (1, 3), (1, 3)],
        event_ts_ms: vec![None; 3],
        events: vec![
            SessionEvent::Output {
                seq: 2,
                data: "out".to_string(),
            },
            SessionEvent::AgentReported {
                seq: 3,
                source: "devboule:stub".to_string(),
                agent: "stub".to_string(),
                state: devboule_protocol::AgentActivityState::Working,
                message: None,
                report_seq: Some(1),
                agent_session_id: None,
                agent_session_path: None,
                session_start_source: None,
            },
            SessionEvent::Recovered { integrity },
        ],
    };
    let runtime = SessionRuntime::from_replay("s.report.cursor".to_string(), None, replay);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.report.cursor",
        Arc::clone(&runtime),
        true,
        Some(0),
        outcome.generation,
        outcome.live_agent_replay,
    );
    let first = {
        let batch = conn.pull_events();
        for event in &batch {
            if matches!(
                event.envelope.event,
                SessionEvent::Recovered { .. } | SessionEvent::Exit { .. }
            ) {
                continue;
            }
            conn.event_sent(event);
        }
        batch
    };
    assert!(first.iter().any(|event| matches!(
        event.envelope.event,
        SessionEvent::AgentReported { seq: 3, .. }
    )));
    let cursor = conn
        .attached
        .lock()
        .expect("attached")
        .get(&conn.id)
        .and_then(|pull| pull.transcript_cursor);
    assert_eq!(
        cursor,
        Some(3),
        "cursor stayed at {cursor:?} after delivering AgentReported seq 3"
    );
    runtime.detach_if_conn(conn.id);
    let conn2 = ConnHandle::new(2);
    let outcome = runtime
        .try_attach_with_replay(None, &conn2, false)
        .expect("reattach");
    conn2.track_with_agent_replay(
        "s.report.cursor",
        Arc::clone(&runtime),
        true,
        cursor,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let second = conn2.pull_events();
    assert!(
        !second
            .iter()
            .any(|event| matches!(event.envelope.event, SessionEvent::AgentReported { .. })),
        "AgentReported was delivered again on reattach: {:?}",
        second
            .iter()
            .map(|event| format!("{:?}", event.envelope.event))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_mid_generation_cursor_pull_costs_the_delta_not_the_total() {
    // The pull attaches at the cursor between the two E1 results, so the
    // rows that moved the running total sit before its first page. The
    // finish it derives for the second result must still be the second
    // turn's delta — the same dollar figure the live reader produced, which
    // is what the latch is for.
    let dir = crate::test_dirs::test_temp_dir("devboule-claude-cost-cursor");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.cost.cursor";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    let frames: Vec<serde_json::Value> = include_str!("../fixtures/wire/claude-e1-results.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("fixture line"))
        .collect();
    for (seq, frame) in frames.iter().enumerate() {
        journal
            .append_blocking(
                crate::journal::acp_envelope_record(
                    session_id,
                    1,
                    u64::try_from(seq).expect("seq") + 1,
                    frame,
                )
                .unwrap(),
            )
            .unwrap();
    }

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.next_seq = 3;
    }
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(
            Some(Cursor {
                generation: runtime.generation(),
                seq: 1,
            }),
            &conn,
            false,
        )
        .expect("attach at the mid-generation cursor");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn);
    let costs: Vec<f64> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentFinished {
                usage: Some(usage), ..
            } => usage.cost_usd,
            _ => None,
        })
        .collect();
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(costs.len(), 1, "one finish on the page: {costs:?}");
    // Binary subtraction rounds, so the delta holds a tolerance.
    assert!(
        (costs[0] - 0.01273).abs() < 1e-9,
        "the second turn's delta, not the cumulative total: {}",
        costs[0]
    );
}
