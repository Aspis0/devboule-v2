//! What a reattach or a late observer is served: preserved pending, the
//! shared backlog, and the manifest exactly once.

use super::super::*;
use super::*;

use serde_json::json;

use super::test_support::drain;

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

                images: Vec::new(),
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
            accepts_images: true,
            provider_id: None,
            model_id: "grok-live".to_string(),
            name: "Grok Live".to_string(),
            provider: None,
            description: None,
            context_tokens: None,
            current_effort: Some("high".to_string()),
            efforts: None,
        }],
        modes: None,
        current_model_provider_id: None,
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

            images: Vec::new(),
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
        None,
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
        None,
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
