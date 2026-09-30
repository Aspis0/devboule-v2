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
