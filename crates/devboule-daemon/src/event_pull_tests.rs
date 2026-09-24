//! Tests for the event pull: live emission, replay ordering and cursor positions.

use super::super::*;
use super::*;
use serde_json::json;

/// Pull until the session queue is empty, recording delivery like the
/// connection writer does.
fn drain(conn: &ConnHandle) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    loop {
        let batch = conn.pull_events();
        if batch.is_empty() {
            return events;
        }
        for event in &batch {
            conn.event_sent(event);
        }
        events.extend(batch.into_iter().map(|pending| pending.envelope.event));
    }
}

fn attach_tracked(runtime: &Arc<SessionRuntime>, conn: &Arc<ConnHandle>) -> u64 {
    let outcome = runtime
        .try_attach_with_replay(None, conn, false)
        .expect("attach");
    let transcript = runtime.is_transcript();
    conn.track_with_agent_replay(
        "s.a.1",
        Arc::clone(runtime),
        transcript,
        Some(0),
        outcome.generation,
        outcome.live_agent_replay,
    );
    outcome.generation
}

#[test]
fn delivered_exit_removes_the_runtime_observer() {
    let runtime = Arc::new(SessionRuntime::new());
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);

    runtime.finish(Some(0));
    let events = drain(&conn);

    assert!(events
        .iter()
        .any(|event| matches!(event, SessionEvent::Exit { .. })));
    assert!(runtime.stream.lock().unwrap().observers.is_empty());
}

#[test]
fn duplicate_subscription_id_does_not_replace_another_session() {
    let conn = ConnHandle::new(1);
    let first = Arc::new(SessionRuntime::new());
    let second = Arc::new(SessionRuntime::new());

    conn.track_with_subscription(7, Arc::clone(&first), false, None, 1, None)
        .expect("first subscription");
    let error = conn
        .track_with_subscription(7, Arc::clone(&second), false, None, 1, None)
        .expect_err("duplicate subscription id must be rejected");
    assert_eq!(error.code, ErrorCode::InvalidRequest);

    let attached = conn.attached.lock().unwrap();
    assert!(Arc::ptr_eq(
        &attached.get(&7).expect("subscription").runtime,
        &first
    ));
}

#[test]
fn a_connection_holds_at_most_sixty_four_subscriptions() {
    // §8 R4's per-connection brake. A peer that attaches the same session
    // sixty-five times is either broken or probing; either way the
    // sixty-fifth is refused rather than booked.
    let conn = ConnHandle::new(1);
    let runtime = Arc::new(SessionRuntime::new());
    for subscription_id in 1..=MAX_SUBSCRIPTIONS as u64 {
        conn.track_with_subscription(subscription_id, Arc::clone(&runtime), false, None, 1, None)
            .expect("a subscription inside the cap is accepted");
    }
    let error = conn
        .track_with_subscription(
            MAX_SUBSCRIPTIONS as u64 + 1,
            Arc::clone(&runtime),
            false,
            None,
            1,
            None,
        )
        .expect_err("the sixty-fifth subscription on one connection is refused");
    assert_eq!(error.code, ErrorCode::CapabilityNotSupported);
    assert_eq!(
        conn.attached.lock().unwrap().len(),
        MAX_SUBSCRIPTIONS,
        "the refused subscription must not be booked"
    );
}

fn live_agent_replay_fixture(
    session_id: &str,
    record: crate::journal::EventRecord,
) -> (
    std::path::PathBuf,
    Arc<Journal>,
    Arc<SessionRuntime>,
    Arc<ConnHandle>,
) {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-agent-replay-parse");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    let next_seq = record.seq.saturating_add(1);
    journal.append_blocking(record).unwrap();

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.next_seq = next_seq;
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
    (dir, journal, runtime, conn)
}

#[test]
fn malformed_agent_report_replay_marks_journal_degraded() {
    let session_id = "s.live.agent.replay.malformed-report";
    let record = crate::journal::EventRecord {
        session_id: session_id.to_string(),
        generation: 1,
        seq: 1,
        kind: crate::journal::EventKind::AgentReport,
        ts_ms: 0,
        payload: b"not a SessionEvent".to_vec(),
    };
    let (dir, journal, runtime, conn) = live_agent_replay_fixture(session_id, record);
    let events = drain(&conn);
    assert!(events
        .iter()
        .any(|event| matches!(event, SessionEvent::JournalDegraded { .. })));
    assert!(runtime.journal_degraded());

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

/// §8b A14 at the egress: the placeholder a provider client writes is
/// replaced with the session's stored origin before the card reaches a
/// subscriber, so a peer session's card cannot be shown as this machine's
/// own — and a session whose origin was never installed surfaces as
/// `unknown`, never as `local`: `local` is measured, never assumed.
#[test]
fn a_published_permission_request_carries_the_sessions_stored_origin() {
    let session_id = "s.live.agent.replay.card-origin";
    let record = crate::journal::EventRecord {
        session_id: session_id.to_string(),
        generation: 1,
        seq: 1,
        kind: crate::journal::EventKind::Output,
        ts_ms: 0,
        payload: b"ready".to_vec(),
    };
    let (dir, journal, runtime, conn) = live_agent_replay_fixture(session_id, record);
    // Clear whatever the attach replayed: this asserts what a *publish*
    // hands the subscriber.
    let _ = drain(&conn);

    // Publish the card a provider client builds — placeholder origin and
    // no idea which device asked for the session — and read it back.
    let publish = |runtime: &SessionRuntime, tool_call_id: &str| {
        runtime.publish_agent_event(
            SessionEvent::PermissionRequest {
                tool_call_id: tool_call_id.to_string(),
                title: "Run command".to_string(),
                description: None,
                command: None,
                args: None,
                cwd: None,
                env: None,
                options: Vec::new(),
                origin: devboule_protocol::SessionOrigin::unknown(),
                create_agent: None,
            },
            None,
        );
    };
    let card_origin = |conn: &ConnHandle| {
        drain(conn)
            .into_iter()
            .find_map(|event| match event {
                SessionEvent::PermissionRequest { origin, .. } => Some(origin),
                _ => None,
            })
            .expect("the subscriber receives the card")
    };

    // Phase one: no stored origin. The placeholder must not survive as
    // `local` — nothing measured this session as this machine's own, so
    // the card says `unknown`. `local` is only ever measured.
    publish(&runtime, "call-unstored");
    assert_eq!(
        card_origin(&conn),
        devboule_protocol::SessionOrigin::unknown(),
        "a session whose origin was never installed reads as unknown, never as local"
    );

    // Phase two: the registry installs the session's stored origin — a
    // paired device's — and the card says that, not the placeholder.
    runtime.set_origin(devboule_protocol::SessionOrigin::peer(
        "device-phone",
        devboule_protocol::PeerRole::Client,
    ));
    publish(&runtime, "call-origin");
    assert_eq!(
        card_origin(&conn),
        devboule_protocol::SessionOrigin::peer("device-phone", devboule_protocol::PeerRole::Client),
        "the placeholder must not survive the egress"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The v9 migration and the replay path together: the bytes the migration
/// writes for a pre-origin permission payload are bytes the live replay
/// parses — no `JournalDegraded`, and the card says `local`.
#[test]
fn a_migrated_permission_payload_replays_as_a_local_card() {
    let session_id = "s.live.agent.replay.migrated-origin";
    // Exactly what a v8 daemon stored for a permission request.
    let legacy = serde_json::to_vec(&serde_json::json!({
        "type": "permission_request",
        "toolCallId": "call-migrated",
        "title": "Run command",
        "options": []
    }))
    .expect("legacy payload");
    let crate::journal::OriginBackfill::Rewritten(payload) =
        crate::journal::payload_with_origin(&legacy)
    else {
        panic!("a pre-origin permission request is what the migration rewrites");
    };
    let record = crate::journal::EventRecord {
        session_id: session_id.to_string(),
        generation: 1,
        seq: 1,
        kind: crate::journal::EventKind::AgentReport,
        ts_ms: 0,
        payload,
    };

    let (dir, journal, runtime, conn) = live_agent_replay_fixture(session_id, record);
    let events = drain(&conn);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::JournalDegraded { .. })),
        "a migrated payload must not degrade the journal: {events:?}"
    );
    let origin = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::PermissionRequest { origin, .. } => Some(origin.clone()),
            _ => None,
        })
        .expect("the replayed card");
    assert_eq!(origin, devboule_protocol::SessionOrigin::local());

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn malformed_acp_envelope_replay_marks_journal_degraded() {
    let session_id = "s.live.agent.replay.malformed-acp";
    let record = crate::journal::EventRecord {
        session_id: session_id.to_string(),
        generation: 1,
        seq: 1,
        kind: crate::journal::EventKind::AcpEnvelope,
        ts_ms: 0,
        payload: b"not JSON".to_vec(),
    };
    let (dir, journal, runtime, conn) = live_agent_replay_fixture(session_id, record);
    let events = drain(&conn);
    assert!(events
        .iter()
        .any(|event| matches!(event, SessionEvent::JournalDegraded { .. })));
    assert!(runtime.journal_degraded());

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unmodeled_acp_envelope_replay_stays_quiet() {
    let session_id = "s.live.agent.replay.unmodeled";
    let record = crate::journal::EventRecord {
        session_id: session_id.to_string(),
        generation: 1,
        seq: 1,
        kind: crate::journal::EventKind::AcpEnvelope,
        ts_ms: 0,
        payload: serde_json::to_vec(&json!({
            "method": "_auth/status_update",
            "params": {"status": "ok"}
        }))
        .unwrap(),
    };
    let (dir, journal, runtime, conn) = live_agent_replay_fixture(session_id, record);
    let events = drain(&conn);
    assert!(!events
        .iter()
        .any(|event| matches!(event, SessionEvent::JournalDegraded { .. })));
    assert!(!runtime.journal_degraded());

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

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
fn same_connection_reattach_preserves_agent_pending_once() {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-agent-same-connection");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.live.agent.same-connection";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
    }
    let conn = ConnHandle::new(1);
    let first = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("first attach");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        first.generation,
        first.live_agent_replay,
    );

    for (message_id, text) in [("m1", "first"), ("m2", "second")] {
        let envelope = json!({
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": message_id,
                    "content": {"type": "text", "text": text}
                }
            }
        });
        let seq = runtime
            .journal_agent_envelope(&envelope)
            .expect("journal pending event");
        runtime.publish_agent_event_with_seq(
            SessionEvent::AgentMessage {
                message_id: Some(message_id.to_string()),
                text: text.to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
            None,
            Some(seq),
        );
    }
    journal.flush().expect("flush pending events");

    let second = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("same-connection reattach");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        second.generation,
        second.live_agent_replay,
    );
    let events = drain(&conn);
    let texts = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentMessage { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(texts, vec!["first", "second"]);
    let stream = runtime.stream.lock().unwrap();
    assert_eq!(
        stream
            .agent_backlog
            .iter()
            .filter(|item| matches!(item, PendingItem::Agent { .. }))
            .count(),
        2
    );
    drop(stream);

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

#[test]
fn live_agent_replay_uses_stored_manifest_state() {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-agent-replay-manifest");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.live.agent.replay.manifest";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    let journal_manifest = json!({
        "method": "_x.ai/models/update",
        "params": {
            "currentModelId": "grok-live",
            "availableModels": [{
                "modelId": "grok-live",
                "name": "Grok Live",
                "_meta": {
                    "supportsReasoningEffort": true,
                    "reasoningEffort": "low",
                    "reasoningEfforts": [{"id": "low", "label": "Low"}, {"id": "high", "label": "High"}]
                }
            }]
        }
    });
    journal
        .append_blocking(
            crate::journal::acp_envelope_record(session_id, 1, 1, &journal_manifest).unwrap(),
        )
        .unwrap();

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: Some("grok".to_string()),
        current_model_id: Some("grok-live".to_string()),
        models: vec![devboule_protocol::SessionModel {
            model_id: "grok-live".to_string(),
            name: "Grok Live".to_string(),
            description: None,
            context_tokens: None,
            current_effort: Some("high".to_string()),
            efforts: None,
        }],
        modes: None,
    });
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.next_seq = 2;
    }
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach live agent");
    // This is the race window: the observer is armed, but replay has not
    // started reading its durable prefix yet.
    let live_manifest = runtime.session_manifest().expect("stored manifest");
    let _ = runtime.publish_agent_event(live_manifest, None);
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn);
    let manifests: Vec<&SessionEvent> = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::SessionManifest { .. }))
        .collect();
    assert_eq!(manifests.len(), 1, "manifest must arrive exactly once");
    let SessionEvent::SessionManifest {
        provider_id,
        current_model_id,
        models,
        ..
    } = manifests[0]
    else {
        unreachable!();
    };
    assert_eq!(provider_id.as_deref(), Some("grok"));
    assert_eq!(current_model_id.as_deref(), Some("grok-live"));
    assert_eq!(models[0].current_effort.as_deref(), Some("high"));

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn live_agent_replay_marks_an_empty_journal_prefix_degraded() {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-agent-replay-empty");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.live.agent.replay.empty";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.next_seq = 2;
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
    let events = drain(&conn);
    assert!(events
        .iter()
        .any(|event| matches!(event, SessionEvent::JournalDegraded { .. })));
    assert!(runtime.journal_degraded());

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn live_agent_attach_keeps_generation_mismatch_loud() {
    let runtime = Arc::new(SessionRuntime::new());
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.generation = 2;
    }
    runtime.generation.store(2, Ordering::Release);
    let conn = ConnHandle::new(1);
    let error = match runtime.try_attach_with_replay(
        Some(Cursor {
            generation: 1,
            seq: 0,
        }),
        &conn,
        true,
    ) {
        Ok(_) => panic!("stale live-agent cursor was accepted"),
        Err(error) => error,
    };
    assert_eq!(error.code, ErrorCode::SessionGenerationMismatch);
}

/// A Reopen resets the client cursor to the new generation's seq 0; the
/// replay that follows must still serve the generations before the
/// attach — the whole transcript, in journal order.
#[test]
fn live_agent_replay_delivers_the_generations_before_the_attach() {
    let dir = crate::test_dirs::test_temp_dir("devboule-cross-gen-replay");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.cross.gen.attach";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    let user_before = SessionEvent::AgentUserMessage {
        message_id: Some("m1".into()),
        text: "gen-1 user".into(),
        author: devboule_protocol::UserMessageAuthor::Human,
        message_kind: devboule_protocol::UserMessageKind::Unknown,
    };
    let answer_before = SessionEvent::AgentMessage {
        message_id: Some("m2".into()),
        text: "gen-1 answer".into(),
        parent_tool_use_id: None,
        spawn_depth: None,
    };
    let answer_after = SessionEvent::AgentMessage {
        message_id: Some("m3".into()),
        text: "after resume".into(),
        parent_tool_use_id: None,
        spawn_depth: None,
    };
    journal
        .append_blocking(
            crate::journal::agent_report_record(session_id, 1, 1, &user_before).unwrap(),
        )
        .unwrap();
    journal
        .append_blocking(
            crate::journal::agent_report_record(session_id, 1, 2, &answer_before).unwrap(),
        )
        .unwrap();
    journal.start_generation(session_id, 2).unwrap();
    journal
        .append_blocking(
            crate::journal::agent_report_record(session_id, 2, 1, &answer_after).unwrap(),
        )
        .unwrap();

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.generation = 2;
        stream.next_seq = 2;
    }
    runtime.generation.store(2, Ordering::Release);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(
            Some(Cursor {
                generation: 2,
                seq: 0,
            }),
            &conn,
            true,
        )
        .expect("attach live agent");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
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
        "the replay must span the resume seam in journal order: {transcript:?}"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Pre-attach history is history: a row from below the attach generation
/// is delivered with its own generation on the envelope and no transcript
/// position, so it can advance neither reader's cursor into a numbering
/// space that is not its own.
#[test]
fn live_agent_replay_stamps_history_envelopes_with_their_own_generation() {
    let dir = crate::test_dirs::test_temp_dir("devboule-history-stamp");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.history.stamp";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    // Generation 1's last record is an AgentReported far above anything
    // generation 2 has published: exactly the shape that corrupts a
    // cursor when it arrives wearing the attach generation.
    let hook_report = SessionEvent::AgentReported {
        seq: 100,
        source: "devboule:stub".to_string(),
        agent: "stub".to_string(),
        state: devboule_protocol::AgentActivityState::Working,
        message: None,
        report_seq: Some(1),
        agent_session_id: None,
        agent_session_path: None,
        session_start_source: None,
    };
    let user_after = SessionEvent::AgentUserMessage {
        message_id: Some("m1".into()),
        text: "after resume".into(),
        author: devboule_protocol::UserMessageAuthor::Human,
        message_kind: devboule_protocol::UserMessageKind::Unknown,
    };
    let answer_after = SessionEvent::AgentMessage {
        message_id: Some("m2".into()),
        text: "gen-2 answer".into(),
        parent_tool_use_id: None,
        spawn_depth: None,
    };
    journal
        .append_blocking(
            crate::journal::agent_report_record(session_id, 1, 100, &hook_report).unwrap(),
        )
        .unwrap();
    journal.start_generation(session_id, 2).unwrap();
    journal
        .append_blocking(
            crate::journal::agent_report_record(session_id, 2, 1, &user_after).unwrap(),
        )
        .unwrap();
    journal
        .append_blocking(
            crate::journal::agent_report_record(session_id, 2, 2, &answer_after).unwrap(),
        )
        .unwrap();

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.generation = 2;
        stream.next_seq = 3;
    }
    runtime.generation.store(2, Ordering::Release);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(
            Some(Cursor {
                generation: 2,
                seq: 0,
            }),
            &conn,
            true,
        )
        .expect("attach live agent");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let mut batch = conn.pull_events();
    while batch.len() < 3 {
        let more = conn.pull_events();
        if more.is_empty() {
            break;
        }
        batch.extend(more);
    }
    let history: Vec<(u64, Option<u64>)> = batch
        .iter()
        .filter(|pending| matches!(pending.envelope.event, SessionEvent::AgentReported { .. }))
        .map(|pending| (pending.envelope.generation, pending.envelope.transcript_seq))
        .collect();
    assert_eq!(
        history,
        vec![(1, None)],
        "a history row must carry its own generation and no transcript position: {history:?}"
    );
    let current: Vec<(u64, Option<u64>)> = batch
        .iter()
        .filter(|pending| matches!(pending.envelope.event, SessionEvent::AgentMessage { .. }))
        .map(|pending| (pending.envelope.generation, pending.envelope.transcript_seq))
        .collect();
    assert_eq!(
        current,
        vec![(2, Some(2))],
        "current-generation rows keep the attach generation and their position: {current:?}"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The daemon's transcript cursor means "how far inside the current
/// generation this reader has got". History must not move it: a reader
/// that accounts a history row as current-generation progress starves
/// its own reattach of every current-generation row below that number.
#[test]
fn history_does_not_move_the_transcript_cursor_or_starve_the_reattach() {
    let integrity = recovered_integrity();
    let replay = crate::journal::Replay {
        generation: 2,
        last_seq: 2,
        integrity,
        event_seqs: vec![(1, 100), (2, 1), (2, 2), (2, 2)],
        events: vec![
            SessionEvent::AgentReported {
                seq: 100,
                source: "devboule:stub".to_string(),
                agent: "stub".to_string(),
                state: devboule_protocol::AgentActivityState::Working,
                message: None,
                report_seq: Some(1),
                agent_session_id: None,
                agent_session_path: None,
                session_start_source: None,
            },
            SessionEvent::AgentUserMessage {
                message_id: Some("m1".into()),
                text: "gen-2 user".into(),
                author: devboule_protocol::UserMessageAuthor::Human,
                message_kind: devboule_protocol::UserMessageKind::Unknown,
            },
            SessionEvent::AgentMessage {
                message_id: Some("m2".into()),
                text: "still here".into(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
            SessionEvent::Recovered { integrity },
        ],
    };
    let runtime = SessionRuntime::from_replay("s.history.cursor".to_string(), None, replay);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.history.cursor",
        Arc::clone(&runtime),
        true,
        Some(0),
        outcome.generation,
        outcome.live_agent_replay,
    );
    // A reader that receives only the first envelope — the history row —
    // and then drops. Whatever cursor it accounts from that one row is
    // what its reattach will present.
    let first = conn.pull_events().remove(0);
    conn.event_sent(&first);
    let accounted = conn
        .attached
        .lock()
        .expect("attached")
        .get(&conn.id)
        .and_then(|pull| pull.transcript_cursor);
    runtime.detach_if_conn(conn.id);

    let conn2 = ConnHandle::new(2);
    let outcome = runtime
        .try_attach_with_replay(None, &conn2, false)
        .expect("reattach");
    conn2.track_with_agent_replay(
        "s.history.cursor",
        Arc::clone(&runtime),
        true,
        accounted,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn2);
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
        vec!["gen-2 user", "still here"],
        "the reattach must still be served the current generation: {transcript:?}"
    );
    // The mechanism, checked after the loss it causes: the accounted
    // cursor stayed at the attach position (0) — history moved it to
    // 100 when the event_sent gate is broken.
    assert_eq!(
        accounted,
        Some(0),
        "history must not advance the transcript cursor"
    );
}

/// One live agent attachment — no screen, no journal — with a single
/// published chat row at `seq`. The queue item is the only place that
/// position exists: no journal row was written under it.
fn live_agent_with_one_chat_row(
    session_id: &str,
    seq: u64,
) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
    let runtime = Arc::new(SessionRuntime::with_journal(session_id.to_string(), None));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.next_seq = seq.saturating_add(1);
        stream.last_applied_seq = seq;
    }
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach live agent");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        Some(0),
        outcome.generation,
        outcome.live_agent_replay,
    );
    runtime.publish_agent_event_with_seq(
        SessionEvent::AgentMessage {
            message_id: Some("live".to_string()),
            text: "live row".to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
        },
        None,
        Some(seq),
    );
    (runtime, conn)
}

fn delivered_chat_row(conn: &ConnHandle) -> PendingEvent {
    conn.pull_events()
        .into_iter()
        .find(|pending| matches!(pending.envelope.event, SessionEvent::AgentMessage { .. }))
        .expect("the live chat row reaches the wire")
}

/// A live row has no journal row behind it yet, so the position the queue
/// carries is the only one its envelope can hold. Without it the row
/// reaches the client positionless, indistinguishable from a marker.
#[test]
fn a_live_agent_row_carries_its_position_on_the_envelope() {
    let (runtime, conn) = live_agent_with_one_chat_row("s.live.position", 7);
    let pending = delivered_chat_row(&conn);
    let wire = serde_json::to_value(&pending.envelope).expect("envelope json");
    assert_eq!(
        wire["transcriptSeq"].as_u64(),
        Some(7),
        "a live row must carry the position its queue handed it: {wire}"
    );
    drop(runtime);
}

/// The transcript cursor is what a reattach presents. A delivered live
/// chat row is progress this reader must keep, or every reconnect replays
/// the conversation from the attach position.
#[test]
fn a_live_chat_row_moves_the_transcript_cursor() {
    let (runtime, conn) = live_agent_with_one_chat_row("s.live.cursor", 7);
    let pending = delivered_chat_row(&conn);
    conn.event_sent(&pending);
    let cursor = conn
        .attached
        .lock()
        .expect("attached")
        .get(&conn.id)
        .and_then(|pull| pull.transcript_cursor);
    assert_eq!(
        cursor,
        Some(7),
        "a delivered live row is progress this reader must keep: {cursor:?}"
    );
    drop(runtime);
}

/// A reattach to a multi-generation transcript with a non-zero cursor is
/// owed the whole history plus the current generation after the cursor.
/// Old-generation rows sit at seqs below the cursor here, which is the
/// case a bare-seq filter silently drops — for both row families: the
/// agent-report map and the in-memory Output chunks.
#[test]
fn reattach_to_history_serves_the_whole_conversation() {
    let integrity = recovered_integrity();
    let replay = crate::journal::Replay {
        generation: 2,
        last_seq: 7,
        integrity,
        event_seqs: vec![(1, 2), (1, 3), (2, 6), (2, 7), (2, 7)],
        events: vec![
            SessionEvent::Output {
                seq: 2,
                data: "gen-1 output".to_string(),
            },
            SessionEvent::AgentUserMessage {
                message_id: Some("m1".into()),
                text: "gen-1 report".into(),
                author: devboule_protocol::UserMessageAuthor::Human,
                message_kind: devboule_protocol::UserMessageKind::Unknown,
            },
            SessionEvent::Output {
                seq: 6,
                data: "gen-2 output".to_string(),
            },
            SessionEvent::AgentMessage {
                message_id: Some("m2".into()),
                text: "still here".into(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
            SessionEvent::Recovered { integrity },
        ],
    };
    let runtime = SessionRuntime::from_replay("s.reattach.history".to_string(), None, replay);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.reattach.history",
        Arc::clone(&runtime),
        true,
        Some(5),
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn);
    let transcript: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { data, .. } => Some(data.clone()),
            SessionEvent::AgentUserMessage { text, .. } => Some(text.clone()),
            SessionEvent::AgentMessage { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        transcript,
        vec!["gen-1 output", "gen-1 report", "gen-2 output", "still here",],
        "history below the cursor is still owed on a reattach: {transcript:?}"
    );
}

/// The journal-copies branch of the transcript replay must serve history
/// too: a runtime whose in-memory scrollback begins above the cursor —
/// hydrated with a catch-up cursor — still owes the older generations'
/// Output rows the journal holds, whatever seq they sit at.
#[test]
fn journal_copies_of_history_survive_the_reattach() {
    let dir = crate::test_dirs::test_temp_dir("devboule-journal-history");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.journal.history";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    // A history Output row the hydrated scrollback does not hold: the
    // runtime below is built from a hand-built replay that starts at
    // the current generation, exactly like a hydrate that arrived with
    // a catch-up cursor.
    journal
        .append_blocking(crate::journal::output_record(
            session_id,
            1,
            2,
            "gen-1 journal row".as_bytes(),
        ))
        .expect("gen-1 journal row");
    journal.start_generation(session_id, 2).unwrap();
    journal
        .append_blocking(crate::journal::output_record(
            session_id,
            2,
            8,
            "journal hole row".as_bytes(),
        ))
        .expect("journal hole row");
    let integrity = recovered_integrity();
    let replay = crate::journal::Replay {
        generation: 2,
        last_seq: 7,
        integrity,
        event_seqs: vec![(2, 7), (2, 7)],
        events: vec![
            SessionEvent::Output {
                seq: 7,
                data: "gen-2 chunk".to_string(),
            },
            SessionEvent::Recovered { integrity },
        ],
    };
    let runtime =
        SessionRuntime::from_replay(session_id.to_string(), Some(Arc::clone(&journal)), replay);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        true,
        Some(5),
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
        vec!["gen-1 journal row", "gen-2 chunk", "journal hole row",],
        "journal copies of history are owed whatever their seq: {ledger:?}"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The stop-tail attach carries the nothing-owed sentinel as its seq: a
/// stop wants no rows, not "everything below the attach generation".
/// History is owed to readers, which is exactly why the sentinel must
/// mean nothing rather than merely a very large seq.
#[test]
fn a_stop_tail_cursor_owes_nothing_not_even_history() {
    let integrity = recovered_integrity();
    let replay = crate::journal::Replay {
        generation: 2,
        last_seq: 1,
        integrity,
        event_seqs: vec![(1, 1), (1, 2), (2, 1), (2, 1)],
        events: vec![
            SessionEvent::Output {
                seq: 1,
                data: "gen-1 ledger".to_string(),
            },
            SessionEvent::AgentUserMessage {
                message_id: Some("m1".into()),
                text: "gen-1 report".into(),
                author: devboule_protocol::UserMessageAuthor::Human,
                message_kind: devboule_protocol::UserMessageKind::Unknown,
            },
            SessionEvent::AgentMessage {
                message_id: Some("m2".into()),
                text: "still here".into(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
            SessionEvent::Recovered { integrity },
        ],
    };
    let runtime = SessionRuntime::from_replay("s.stop.tail".to_string(), None, replay);

    // The store is not degenerate: a fresh reader is served the history.
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.stop.tail",
        Arc::clone(&runtime),
        true,
        Some(0),
        outcome.generation,
        outcome.live_agent_replay,
    );
    let seen = drain(&conn);
    assert!(!seen.is_empty(), "the fixture must hold rows");

    // The stop-tail reader: the cursor the client's stop sends.
    let conn2 = ConnHandle::new(2);
    let outcome = runtime
        .try_attach_with_replay(
            Some(Cursor {
                generation: 2,
                seq: u64::MAX,
            }),
            &conn2,
            false,
        )
        .expect("stop-tail attach");
    conn2.track_with_agent_replay(
        "s.stop.tail",
        Arc::clone(&runtime),
        true,
        Some(u64::MAX),
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn2);
    let transcript: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { data, .. } => Some(data.clone()),
            SessionEvent::AgentUserMessage { text, .. } => Some(text.clone()),
            SessionEvent::AgentMessage { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        transcript,
        Vec::<String>::new(),
        "a stop-tail cursor must owe nothing, not the history: {transcript:?}"
    );
}

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
        event_seqs: vec![(1, 1), (1, 2), (2, 1)],
        events: vec![
            SessionEvent::AgentUserMessage {
                message_id: Some("m1".into()),
                text: "gen-1 user".into(),
                author: devboule_protocol::UserMessageAuthor::Human,
                message_kind: devboule_protocol::UserMessageKind::Unknown,
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

#[test]
fn live_claude_replay_derives_journaled_views() {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-claude-replay");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.live.claude.replay";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Claude,
            "Claude",
        ))
        .unwrap();
    let init = json!({
        "type": "system",
        "subtype": "init",
        "session_id": "claude-peer",
        "model": "claude-test"
    });
    let assistant = json!({
        "type": "assistant",
        "message": {
            "id": "message-1",
            "model": "claude-test",
            "content": [{"type": "text", "text": "claude replay"}]
        }
    });
    journal
        .append_blocking(crate::journal::acp_envelope_record(session_id, 1, 1, &init).unwrap())
        .unwrap();
    journal
        .append_blocking(crate::journal::acp_envelope_record(session_id, 1, 2, &assistant).unwrap())
        .unwrap();

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: Some("claude".to_string()),
        current_model_id: Some("claude-test".to_string()),
        models: vec![devboule_protocol::SessionModel {
            model_id: "claude-test".to_string(),
            name: "Claude Test".to_string(),
            description: None,
            context_tokens: None,
            current_effort: None,
            efforts: None,
        }],
        modes: None,
    });
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.next_seq = 3;
    }
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach live Claude session");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let envelopes = {
        let mut envelopes = Vec::new();
        loop {
            let batch = conn.pull_events();
            if batch.is_empty() {
                break envelopes;
            }
            for event in &batch {
                conn.event_sent(event);
            }
            envelopes.extend(batch.into_iter().map(|pending| pending.envelope));
        }
    };
    let events = envelopes
        .iter()
        .map(|envelope| envelope.event.clone())
        .collect::<Vec<_>>();
    let manifest = envelopes
        .iter()
        .find(|envelope| matches!(envelope.event, SessionEvent::SessionManifest { .. }))
        .expect("replay emits the stored manifest");
    assert_eq!(
        manifest.transcript_seq, None,
        "a manifest summarizes runtime state; it is not a transcript position"
    );
    assert!(events.iter().any(|event| {
            matches!(event, SessionEvent::SessionManifest { current_model_id, .. } if current_model_id.as_deref() == Some("claude-test"))
        }));
    assert!(events.iter().any(|event| {
        matches!(event, SessionEvent::AgentMessage { text, .. } if text == "claude replay")
    }));

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn poisoned_stream_is_dead_and_not_reused() {
    let runtime = Arc::new(SessionRuntime::new());
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);
    let poisoned = Arc::clone(&runtime);
    let panic = std::thread::spawn(move || {
        let _stream = poisoned.stream.lock().expect("stream lock");
        panic!("simulate a terminal-state panic");
    });
    assert!(panic.join().is_err());

    runtime.publish_output("must not be applied");
    let events = drain(&conn);
    assert!(runtime.terminal_dead.load(Ordering::Acquire));
    assert!(matches!(
        events.as_slice(),
        [
            SessionEvent::JournalDegraded {
                dropped_frames: 0,
                dropped_bytes: 0,
            },
            SessionEvent::Exit { code: None }
        ]
    ));
    assert!(matches!(
        runtime.try_attach_with_replay(None, &conn, false),
        Err(error) if error == process_gone()
    ));
}

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
fn third_live_agent_observer_keeps_the_shared_backlog() {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-agent-delayed-observer");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let runtime = Arc::new(SessionRuntime::with_journal(
        "s.live.agent.delayed-observer".to_string(),
        Some(Arc::clone(&journal)),
    ));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.next_seq = 3;
        let event = SessionEvent::AgentMessage {
            message_id: Some("delayed".to_string()),
            text: "must survive".to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
        };
        let bytes = serde_json::to_vec(&event).unwrap().len();
        stream.agent_backlog.push_back(PendingItem::Agent {
            seq: Some(2),
            event,
            bytes,
        });
        stream.agent_backlog_bytes = bytes;
        stream.agent_backlog_frames = 1;
    }

    let fast = ConnHandle::new(1);
    let fast_outcome = runtime
        .try_attach_with_subscription(
            101,
            Some(Cursor {
                generation: 1,
                seq: 2,
            }),
            &fast,
            true,
        )
        .expect("fast observer attaches");
    fast.track_with_agent_replay(
        "s.live.agent.delayed-observer",
        Arc::clone(&runtime),
        false,
        None,
        fast_outcome.generation,
        fast_outcome.live_agent_replay,
    );
    runtime.finish_live_agent_replay(
        AttachmentKey {
            conn_id: fast.id,
            subscription_id: 101,
        },
        2,
        &std::collections::HashSet::new(),
    );

    let middle = ConnHandle::new(2);
    let middle_outcome = runtime
        .try_attach_with_subscription(
            202,
            Some(Cursor {
                generation: 1,
                seq: 2,
            }),
            &middle,
            true,
        )
        .expect("middle observer attaches");
    middle.track_with_agent_replay(
        "s.live.agent.delayed-observer",
        Arc::clone(&runtime),
        false,
        None,
        middle_outcome.generation,
        middle_outcome.live_agent_replay,
    );

    let delayed = ConnHandle::new(3);
    let delayed_outcome = runtime
        .try_attach_with_subscription(
            303,
            Some(Cursor {
                generation: 1,
                seq: 0,
            }),
            &delayed,
            true,
        )
        .expect("delayed observer attaches");
    delayed.track_with_agent_replay(
        "s.live.agent.delayed-observer",
        Arc::clone(&runtime),
        false,
        None,
        delayed_outcome.generation,
        delayed_outcome.live_agent_replay,
    );
    runtime.finish_live_agent_replay(
        AttachmentKey {
            conn_id: delayed.id,
            subscription_id: 303,
        },
        0,
        &std::collections::HashSet::new(),
    );

    let stream = runtime.stream.lock().unwrap();
    assert!(stream
        .observers
        .get(&AttachmentKey {
            conn_id: delayed.id,
            subscription_id: 303,
        })
        .unwrap()
        .pending
        .iter()
        .any(|item| matches!(
            item,
            PendingItem::Agent {
                event: SessionEvent::AgentMessage { text, .. },
                ..
            } if text == "must survive"
        )));

    drop(stream);
    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

fn recovered_integrity() -> TranscriptIntegrity {
    TranscriptIntegrity::Unverifiable {
        dropped_frames: 0,
        dropped_bytes: 0,
        trimmed_bytes: 0,
    }
}

#[test]
fn recovered_acp_views_must_not_vanish_behind_a_high_output_cursor() {
    let integrity = recovered_integrity();
    let replay = crate::journal::Replay {
        generation: 1,
        last_seq: 11,
        integrity,
        event_seqs: vec![(1, 10), (1, 11), (1, 11)],
        events: vec![
            SessionEvent::Output {
                seq: 10,
                data: "shell".to_string(),
            },
            SessionEvent::AgentThought {
                message_id: None,
                text: "The".to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
            SessionEvent::Recovered { integrity },
        ],
    };
    let runtime = SessionRuntime::from_replay("s.acp.replay".to_string(), None, replay);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.acp.replay",
        Arc::clone(&runtime),
        true,
        Some(10),
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn);
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentThought { text, .. } if text == "The"
        )),
        "reattach after seq 10 dropped the ACP thought: {events:?}"
    );
}

#[test]
fn transcript_cursor_advances_past_agent_reported() {
    let integrity = recovered_integrity();
    let replay = crate::journal::Replay {
        generation: 1,
        last_seq: 3,
        integrity,
        event_seqs: vec![(1, 2), (1, 3), (1, 3)],
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
fn live_attach_ends_with_snapshot_output_then_exit() {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.publish_output("before");
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);
    runtime.publish_output("after");
    runtime.finish(Some(0));
    let events = drain(&conn);
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| match event {
            SessionEvent::Snapshot { .. } => "snapshot",
            SessionEvent::Output { .. } => "output",
            SessionEvent::Exit { .. } => "exit",
            SessionEvent::Recovered { .. } => "recovered",
            SessionEvent::Detached => "detached",
            SessionEvent::Silent { .. } => "silent",
            SessionEvent::JournalDegraded { .. } => "journal_degraded",
            SessionEvent::SessionsSnapshot { .. } => "sessions_snapshot",
            SessionEvent::AgentMessage { .. } => "agent_message",
            SessionEvent::AgentUserMessage { .. } => "agent_user_message",
            SessionEvent::Steered { .. } => "steered",
            SessionEvent::AgentThought { .. } => "agent_thought",
            SessionEvent::AvailableCommands { .. } => "available_commands",
            SessionEvent::AgentToolCall { .. } => "agent_tool_call",
            SessionEvent::AgentToolUpdate { .. } => "agent_tool_update",
            SessionEvent::AgentFinished { .. } => "agent_finished",
            SessionEvent::AgentTaskStarted { .. } => "agent_task_started",
            SessionEvent::AgentTaskNotification { .. } => "agent_task_notification",
            SessionEvent::AgentBackgroundTasksChanged { .. } => "agent_background_tasks_changed",
            SessionEvent::AgentError { .. } => "agent_error",
            SessionEvent::AgentStderr { .. } => "agent_stderr",
            SessionEvent::PermissionRequest { .. } => "permission_request",
            SessionEvent::PermissionResolved { .. } => "permission_resolved",
            SessionEvent::PermissionAnswered { .. } => "permission_answered",
            SessionEvent::SessionManifest { .. } => "session_manifest",
            SessionEvent::SessionNotice { .. } => "session_notice",
            SessionEvent::AgentReported { .. } => "agent_reported",
            SessionEvent::AgentCreated { .. } => "agent_created",
            SessionEvent::ChildFinished { .. } => "child_finished",
            SessionEvent::ContextUsage { .. } => "context_usage",
            SessionEvent::PlanUsage { .. } => "plan_usage",
        })
        .collect();
    assert_eq!(kinds, ["snapshot", "output", "exit"]);
    // Exit must not overtake queued output.
    assert!(matches!(
        events.last(),
        Some(SessionEvent::Exit { code: Some(0) })
    ));
}

#[test]
fn live_terminal_positions_survive_connection_replacement() {
    let runtime = Arc::new(SessionRuntime::new());
    let conn = ConnHandle::new(1);
    let generation = attach_tracked(&runtime, &conn);

    let initial = conn.pull_events();
    let snapshot = initial
        .iter()
        .find_map(|event| match event.envelope.event {
            SessionEvent::Snapshot { as_of_seq, .. } => {
                Some((as_of_seq, event.envelope.transcript_seq))
            }
            _ => None,
        })
        .expect("live terminal attach has a snapshot");
    assert_eq!(
        snapshot.1,
        Some(snapshot.0),
        "the live snapshot must carry its stream position"
    );
    for event in &initial {
        conn.event_sent(event);
    }

    runtime.publish_output("before replacement");
    let first = conn.pull_events();
    let output_seq = first
        .iter()
        .find_map(|event| match event.envelope.event {
            SessionEvent::Output { seq, .. } => Some((seq, event.envelope.transcript_seq)),
            _ => None,
        })
        .expect("live terminal output reaches the connection");
    assert_eq!(
        output_seq.1,
        Some(output_seq.0),
        "live output must carry its stream position"
    );
    for event in &first {
        conn.event_sent(event);
    }
    let cursor = conn
        .attached
        .lock()
        .expect("attached")
        .get(&conn.id)
        .and_then(|pull| pull.transcript_cursor)
        .expect("live output advances the cursor");
    assert_eq!(cursor, output_seq.0);

    runtime.detach_if_conn(conn.id);
    let replacement = ConnHandle::new(2);
    let outcome = runtime
        .try_attach_with_replay(
            Some(Cursor {
                generation,
                seq: cursor,
            }),
            &replacement,
            false,
        )
        .expect("replacement attach");
    replacement.track_with_agent_replay(
        "s.a.1",
        Arc::clone(&runtime),
        false,
        Some(cursor),
        outcome.generation,
        outcome.live_agent_replay,
    );
    runtime.publish_output("after replacement");
    let second = replacement.pull_events();
    let output_seqs = second
        .iter()
        .filter_map(|event| match event.envelope.event {
            SessionEvent::Output { seq, .. } => Some(seq),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        output_seqs,
        vec![output_seq.0 + 1],
        "replacement must deliver only output after the live cursor"
    );
}

#[test]
fn journal_keeps_every_frame_for_recovery() {
    let dir = crate::test_dirs::test_temp_dir("devboule-recovery");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    journal
        .upsert_blocking(new_session_record(
            "s.recover.1",
            "S-1-5-21-1",
            None,
            SessionKind::Terminal,
            "Terminal",
        ))
        .unwrap();
    let runtime = Arc::new(SessionRuntime::with_journal(
        "s.recover.1".into(),
        Some(Arc::clone(&journal)),
    ));
    let payload = "x".repeat(8192);
    for _ in 1..=300 {
        runtime.publish_output(&payload);
    }
    runtime.finish(Some(0));
    journal.flush().unwrap();

    let replay = journal.replay("s.recover.1").unwrap();
    let seqs: Vec<u64> = replay
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { seq, .. } => Some(*seq),
            _ => None,
        })
        .collect();
    assert_eq!(seqs, (1..=300).collect::<Vec<_>>());

    // The recovered runtime is a transcript: no emulator, journal replay
    // instead of a snapshot. Hydration reads unpositioned — the store
    // holds every frame, whatever a reattaching cursor claims — so the
    // attach below serves the whole transcript from the store alone.
    let replay = journal.replay("s.recover.1").unwrap();
    let recovered =
        SessionRuntime::from_replay("s.recover.1".into(), Some(Arc::clone(&journal)), replay);
    assert!(recovered.is_transcript());
    assert_eq!(recovered.transcript_chunks().len(), 300);
    let conn = ConnHandle::new(1);
    attach_tracked(&recovered, &conn);
    let events = drain(&conn);
    let seqs: Vec<u64> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { seq, .. } => Some(*seq),
            _ => None,
        })
        .collect();
    assert_eq!(seqs, (1..=300).collect::<Vec<_>>());
    assert_eq!(
        recovered.journal_replay_count(),
        0,
        "zero journal reads: the store alone served the attach (the \
             counter spans both read kinds, so zero means neither ran)"
    );
    drop(recovered);
    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn next_exit_wake_is_zero_once_the_drain_has_elapsed() {
    let runtime = Arc::new(SessionRuntime::new());
    let conn = ConnHandle::new(1);
    let outcome = runtime.try_attach_with_replay(None, &conn, false).unwrap();
    conn.track_with_agent_replay(
        "s.a.1",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    assert_eq!(conn.next_exit_wake(), None);
    runtime.mark_exited(Some(0));
    let wake = conn.next_exit_wake().expect("drain timer");
    assert!(wake <= EXIT_DRAIN);
    std::thread::sleep(EXIT_DRAIN + Duration::from_millis(10));
    assert_eq!(conn.next_exit_wake(), Some(Duration::ZERO));
    let events = conn.pull_events();
    assert!(
        events
            .iter()
            .any(|envelope| matches!(envelope.envelope.event, SessionEvent::Exit { .. })),
        "zero wake must let the writer emit Exit, got {events:?}"
    );
    for event in &events {
        conn.event_sent(event);
    }
}

#[test]
fn live_journal_degradation_reaches_attached_client_once() {
    let runtime = Arc::new(SessionRuntime::new());
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);

    runtime.publish_output("still live");
    runtime.mark_journal_degraded();
    runtime.mark_journal_degraded();

    let events = drain(&conn);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::JournalDegraded { .. }))
            .count(),
        1,
        "degradation must be delivered exactly once: {events:?}"
    );
    assert!(events
        .iter()
        .all(|event| !matches!(event, SessionEvent::Exit { .. })));
    assert!(!runtime.stream.lock().unwrap().process_exited);
}

#[test]
fn recovered_pull_ends_with_recovered_not_exit() {
    let replay = crate::journal::Replay {
        generation: 1,
        last_seq: 1,
        integrity: TranscriptIntegrity::Unverifiable {
            dropped_frames: 0,
            dropped_bytes: 0,
            trimmed_bytes: 0,
        },
        event_seqs: vec![(1, 1), (1, 1)],
        events: vec![
            SessionEvent::Output {
                seq: 1,
                data: "hello".to_string(),
            },
            SessionEvent::Recovered {
                integrity: TranscriptIntegrity::Unverifiable {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                    trimmed_bytes: 0,
                },
            },
        ],
    };
    let runtime = SessionRuntime::from_replay("s.a.1".to_string(), None, replay);
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);
    let events = drain(&conn);
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| match event {
            SessionEvent::Output { .. } => "output",
            SessionEvent::Exit { .. } => "exit",
            SessionEvent::Recovered { .. } => "recovered",
            SessionEvent::Detached => "detached",
            SessionEvent::Silent { .. } => "silent",
            SessionEvent::JournalDegraded { .. } => "journal_degraded",
            SessionEvent::SessionsSnapshot { .. } => "sessions_snapshot",
            SessionEvent::Snapshot { .. } => "snapshot",
            SessionEvent::AgentMessage { .. } => "agent_message",
            SessionEvent::AgentUserMessage { .. } => "agent_user_message",
            SessionEvent::Steered { .. } => "steered",
            SessionEvent::AgentThought { .. } => "agent_thought",
            SessionEvent::AvailableCommands { .. } => "available_commands",
            SessionEvent::AgentToolCall { .. } => "agent_tool_call",
            SessionEvent::AgentToolUpdate { .. } => "agent_tool_update",
            SessionEvent::AgentFinished { .. } => "agent_finished",
            SessionEvent::AgentTaskStarted { .. } => "agent_task_started",
            SessionEvent::AgentTaskNotification { .. } => "agent_task_notification",
            SessionEvent::AgentBackgroundTasksChanged { .. } => "agent_background_tasks_changed",
            SessionEvent::AgentError { .. } => "agent_error",
            SessionEvent::AgentStderr { .. } => "agent_stderr",
            SessionEvent::PermissionRequest { .. } => "permission_request",
            SessionEvent::PermissionResolved { .. } => "permission_resolved",
            SessionEvent::PermissionAnswered { .. } => "permission_answered",
            SessionEvent::SessionManifest { .. } => "session_manifest",
            SessionEvent::SessionNotice { .. } => "session_notice",
            SessionEvent::AgentReported { .. } => "agent_reported",
            SessionEvent::AgentCreated { .. } => "agent_created",
            SessionEvent::ChildFinished { .. } => "child_finished",
            SessionEvent::ContextUsage { .. } => "context_usage",
            SessionEvent::PlanUsage { .. } => "plan_usage",
        })
        .collect();
    assert_eq!(kinds, ["output", "recovered"]);
}
