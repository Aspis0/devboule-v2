//! The views a replay re-derives from journal rows, and must not drop.

use super::super::*;
use super::*;

use serde_json::json;

use super::test_support::{drain, recovered_integrity};

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
