//! The live-agent replay walk: complete, ordered, deduplicated, and paged
//! under the pull budget.

use super::super::*;
use super::*;

use serde_json::json;

use super::test_support::drain;

#[test]
fn live_agent_replay_is_complete_ordered_deduplicated_and_not_pending() {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-agent-replay");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.live.agent.replay";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    let first_envelope = json!({
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "messageId": "m1",
                "content": {"type": "text", "text": "first"}
            }
        }
    });
    let second_envelope = json!({
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_thought_chunk",
                "content": {"type": "text", "text": "second"}
            }
        }
    });
    journal
        .append_blocking(
            crate::journal::acp_envelope_record(session_id, 1, 1, &first_envelope).unwrap(),
        )
        .unwrap();
    journal
        .append_blocking(
            crate::journal::acp_envelope_record(session_id, 1, 2, &second_envelope).unwrap(),
        )
        .unwrap();

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    runtime.publish_agent_event_with_seq(
        SessionEvent::AgentMessage {
            message_id: Some("m1".to_string()),
            text: "first".to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,

            images: Vec::new(),
        },
        None,
        Some(1),
    );
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.next_seq = 3;
    }
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach live agent");
    assert_eq!(
        outcome
            .live_agent_replay
            .as_ref()
            .map(|replay| replay.watermark),
        Some(2)
    );
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let stream = runtime.stream.lock().unwrap();
    assert_eq!(
        stream
            .observers
            .get(&AttachmentKey {
                conn_id: conn.id,
                subscription_id: conn.id,
            })
            .expect("attachment")
            .pending
            .len(),
        0
    );
    drop(stream);

    runtime.publish_agent_event(
        SessionEvent::AgentMessage {
            message_id: Some("m2".to_string()),
            text: "live".to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,

            images: Vec::new(),
        },
        None,
    );
    let events = drain(&conn);
    assert_eq!(
        events,
        vec![
            SessionEvent::AgentMessage {
                message_id: Some("m1".to_string()),
                text: "first".to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,

                images: Vec::new(),
            },
            SessionEvent::AgentThought {
                message_id: None,
                text: "second".to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
            SessionEvent::AgentMessage {
                message_id: Some("m2".to_string()),
                text: "live".to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,

                images: Vec::new(),
            },
        ]
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                SessionEvent::AgentMessage { text, .. } if text == "first"
            ))
            .count(),
        1
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn live_agent_replay_pages_without_filling_any_stream_queue() {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-agent-replay-pages");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.live.agent.replay.pages";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    let total = PULL_BATCH as u64 * 2 + 1;
    for seq in 1..=total {
        let envelope = json!({
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": format!("m-{seq}"),
                    "content": {"type": "text", "text": format!("history-{seq}")}
                }
            }
        });
        journal
            .append_blocking(
                crate::journal::acp_envelope_record(session_id, 1, seq, &envelope).unwrap(),
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
        stream.next_seq = total + 1;
    }
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach live agent");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let stream = runtime.stream.lock().unwrap();
    assert_eq!(
        stream
            .observers
            .get(&AttachmentKey {
                conn_id: conn.id,
                subscription_id: conn.id,
            })
            .expect("attachment")
            .pending
            .len(),
        0
    );
    drop(stream);

    let first = conn.pull_events();
    assert!(!first.is_empty());
    assert!(first.len() <= PULL_BATCH);
    let stream = runtime.stream.lock().unwrap();
    assert_eq!(
        stream
            .observers
            .get(&AttachmentKey {
                conn_id: conn.id,
                subscription_id: conn.id,
            })
            .expect("attachment")
            .pending
            .len(),
        0
    );
    drop(stream);
    let replay_pending = conn
        .attached
        .lock()
        .unwrap()
        .get(&conn.id)
        .and_then(|pull| pull.agent_replay.as_ref())
        .map(|replay| replay.pending.len())
        .unwrap_or(0);
    assert!(
        replay_pending <= PULL_BATCH,
        "replay page exceeded pull budget: {replay_pending}"
    );

    let mut events = Vec::new();
    for event in &first {
        conn.event_sent(event);
    }
    events.extend(first.into_iter().map(|pending| pending.envelope.event));
    events.extend(drain(&conn));
    let texts: Vec<String> = events
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::AgentMessage { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(texts.len(), total as usize);
    for (index, text) in texts.into_iter().enumerate() {
        assert_eq!(text, format!("history-{}", index + 1));
    }

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn live_agent_replay_recovers_live_tail_after_pending_overflow() {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-agent-replay-tail");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.live.agent.replay.tail";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    let history = 80_u64;
    for seq in 1..=history {
        let envelope = json!({
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": format!("history-{seq}"),
                    "content": {"type": "text", "text": format!("history-{seq}")}
                }
            }
        });
        journal
            .append_blocking(
                crate::journal::acp_envelope_record(session_id, 1, seq, &envelope).unwrap(),
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
        stream.next_seq = history + 1;
    }
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach live agent");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );

    let first = conn.pull_events();
    for event in &first {
        conn.event_sent(event);
    }
    let mut events = first
        .into_iter()
        .map(|pending| pending.envelope.event)
        .collect::<Vec<_>>();

    for seq in 1..=80_u64 {
        let envelope = json!({
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": format!("live-{seq}"),
                    "content": {"type": "text", "text": format!("live-{seq}")}
                }
            }
        });
        let journal_seq = runtime
            .journal_agent_envelope(&envelope)
            .expect("journal live tail envelope");
        assert_eq!(journal_seq, history + seq);
        runtime.publish_agent_event_with_seq(
            SessionEvent::AgentMessage {
                message_id: Some(format!("live-{seq}")),
                text: format!("live-{seq}"),
                parent_tool_use_id: None,
                spawn_depth: None,

                images: Vec::new(),
            },
            None,
            Some(journal_seq),
        );
    }
    journal.flush().expect("flush live tail envelopes");
    events.extend(drain(&conn));

    let texts: Vec<String> = events
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::AgentMessage { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    let expected: Vec<String> = (1..=history)
        .map(|seq| format!("history-{seq}"))
        .chain((1..=80).map(|seq| format!("live-{seq}")))
        .collect();
    assert_eq!(texts, expected);

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}
