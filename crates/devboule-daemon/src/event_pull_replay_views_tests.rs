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

/// The app's attach road on a legacy Claude session: the rows re-derive the
/// reading, and the seam delivers the stored manifest instead of the journal's.
#[test]
fn a_legacy_claude_replay_delivers_the_reading_and_the_windowless_manifest() {
    let dir = crate::test_dirs::test_temp_dir("devboule-legacy-claude-window");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.live.claude.legacywindow";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Claude,
            "Claude",
        ))
        .unwrap();
    // The legacy row: the measured result without its per-model usage.
    let mut result: serde_json::Value = include_str!("../fixtures/wire/claude-e1-results.jsonl")
        .lines()
        .next()
        .expect("first measured result")
        .parse()
        .expect("fixture line");
    result
        .as_object_mut()
        .expect("envelope object")
        .remove("modelUsage");
    let init = json!({
        "type": "system",
        "subtype": "init",
        "session_id": "claude-peer",
        "model": "claude-opus-5[1m]"
    });
    journal
        .append_blocking(crate::journal::acp_envelope_record(session_id, 1, 1, &init).unwrap())
        .unwrap();
    journal
        .append_blocking(crate::journal::acp_envelope_record(session_id, 1, 2, &result).unwrap())
        .unwrap();

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    // As the live road stores it: the view's own init manifest.
    for event in crate::claude_view::ClaudeView::new(None).ingest(&init) {
        if matches!(event, SessionEvent::SessionManifest { .. }) {
            runtime.store_session_manifest(event);
        }
    }
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.next_seq = 3;
    }
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach legacy Claude session");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn);

    let contexts: Vec<&SessionEvent> = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::ContextUsage { .. }))
        .collect();
    let [context] = contexts.as_slice() else {
        panic!("one reading from the legacy result, got {}", contexts.len());
    };
    assert_eq!(
        **context,
        SessionEvent::ContextUsage {
            model_id: Some("claude-opus-5[1m]".to_string()),
            used_tokens: 22_826,
            max_tokens: None,
            live: false,
        },
        "a legacy row carries no window"
    );
    let manifests: Vec<&SessionEvent> = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::SessionManifest { .. }))
        .collect();
    let [manifest] = manifests.as_slice() else {
        panic!(
            "the stored manifest arrives exactly once, got {}",
            manifests.len()
        );
    };
    let SessionEvent::SessionManifest { models, .. } = manifest else {
        unreachable!();
    };
    assert_eq!(models.len(), 1, "one model in the stored manifest");
    assert_eq!(models[0].model_id, "claude-opus-5[1m]");
    assert_eq!(
        models[0].context_tokens, None,
        "the stored Claude manifest names no window"
    );

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
        event_ts_ms: vec![None; 3],
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

/// pi's withheld-finish marker must hold across a page boundary: the marker
/// is the last row of one page, the `turn_end` it owns — written straight
/// after it, no frame between — the first row of the next, and the finish
/// the live pass withheld must not re-derive on reattach — the same event
/// set the restart hydrate derives through `pi_view::drive_replay`.
#[test]
fn live_pi_replay_honours_the_withheld_finish_across_a_page_boundary() {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-pi-withheld-replay");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.live.pi.withheld";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Pi,
            "Pi",
        ))
        .unwrap();
    // Fillers derive nothing; they push the marker onto the last slot of
    // the first page (a page is PULL_BATCH rows).
    for seq in 1..PULL_BATCH as u64 {
        journal
            .append_blocking(
                crate::journal::acp_envelope_record(
                    session_id,
                    1,
                    seq,
                    &json!({"type":"pi_filler"}),
                )
                .unwrap(),
            )
            .unwrap();
    }
    journal
        .append_blocking(
            crate::journal::acp_envelope_record(
                session_id,
                1,
                PULL_BATCH as u64,
                &crate::claude_view::withheld_finish_marker(),
            )
            .unwrap(),
        )
        .unwrap();
    let turn_end = json!({
        "type": "turn_end",
        "message": {
            "role": "assistant",
            "content": [],
            "model": "pi-test",
            "usage": {"totalTokens": 42},
            "stopReason": "end_turn"
        },
        "toolResults": []
    });
    journal
        .append_blocking(
            crate::journal::acp_envelope_record(session_id, 1, PULL_BATCH as u64 + 1, &turn_end)
                .unwrap(),
        )
        .unwrap();

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    runtime.set_agent_kind(SessionKind::Pi);
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.next_seq = PULL_BATCH as u64 + 2;
    }
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach live pi session");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentFinished { .. })),
        "the finish the marker withheld must not re-derive across the page boundary: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::ContextUsage {
                used_tokens: 42,
                ..
            }
        )),
        "the turn_end's remaining events still derive: {events:?}"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The pi checklist is one of the views the incremental reattach road
/// re-derives: a `tool_execution_end` envelope goes through
/// `pi_view::drive_replay`, which runs the task adapter, so the paged
/// journal replay carries the same `AgentTasks` the live dispatch published.
#[test]
fn live_pi_replay_derives_the_journaled_checklist() {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-pi-checklist-replay");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.live.pi.checklist";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Pi,
            "Pi",
        ))
        .unwrap();
    let end = json!({
        "type": "tool_execution_end",
        "toolCallId": "call_23432",
        "toolName": "set_goal_tasks",
        "isError": false,
        "result": {
            "content": [{"type": "text", "text": "Task list set and confirmed. 2 tasks."}],
            "details": {"version": 3, "goal": {"taskList": {"tasks": [
                {"id": "task-1", "title": "Inspect workspace", "status": "pending"},
                {"id": "task-2", "title": "Summarize findings", "status": "pending"}
            ]}}}
        }
    });
    journal
        .append_blocking(crate::journal::acp_envelope_record(session_id, 1, 1, &end).unwrap())
        .unwrap();

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    runtime.set_agent_kind(SessionKind::Pi);
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.next_seq = 2;
    }
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach live pi session");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn);
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentTasks { items }
                if items.len() == 2
                    && items[0].text == "Inspect workspace"
                    && items[1].text == "Summarize findings"
        )),
        "the incremental attach re-derives the checklist from the envelope: {events:?}"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A refused second row can strand a marker. This fixture walks the paged
/// replay over marker, unrelated envelope, `turn_end`: the intervening
/// envelope expires the marker, so the later finish derives and the
/// unrelated envelope's own event still arrives.
///
/// Assumption: the marker/`turn_end` pair is normally written adjacently. A
/// stranded marker that meets a genuine `turn_end` directly would still
/// suppress that finish, losing it and its usage — the same drop live
/// suppression makes. The adjacent case is pinned one layer down in
/// `pi_view`, the page-boundary case in the test above.
#[test]
fn live_pi_replay_expires_a_stranded_marker_after_an_intervening_frame() {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-pi-stranded-marker");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.live.pi.stranded";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Pi,
            "Pi",
        ))
        .unwrap();
    let unrelated = json!({
        "type": "message_update",
        "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "later"}
    });
    let turn_end = json!({
        "type": "turn_end",
        "message": {
            "role": "assistant",
            "content": [],
            "model": "pi-test",
            "usage": {"totalTokens": 42},
            "stopReason": "end_turn"
        },
        "toolResults": []
    });
    for (seq, value) in [
        (1, &crate::claude_view::withheld_finish_marker()),
        (2, &unrelated),
        (3, &turn_end),
    ] {
        journal
            .append_blocking(
                crate::journal::acp_envelope_record(session_id, 1, seq, value).unwrap(),
            )
            .unwrap();
    }

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    runtime.set_agent_kind(SessionKind::Pi);
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.next_seq = 4;
    }
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach live pi session");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentFinished { .. })),
        "a non-`turn_end` frame expires the stranded marker before this later finish; a stranded marker meeting a `turn_end` directly would still be suppressed, losing that finish and its usage: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentMessage { text, .. } if text == "later"
        )),
        "the unrelated envelope still derives: {events:?}"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A resume seam is a new provider process: a marker the previous
/// generation stranded must not suppress the new generation's own
/// `turn_end` — the bit resets with the other per-generation parser state,
/// as the restart hydrate scopes its copy inside the generation loop.
#[test]
fn live_pi_replay_drops_the_withheld_marker_at_a_resume_seam() {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-pi-marker-resume");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.live.pi.resume";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Pi,
            "Pi",
        ))
        .unwrap();
    // Generation 1 ends on the marker; generation 2 opens with its own
    // turn_end. The pair cannot be adjacent across the seam — the marker
    // belongs to the process that ended.
    journal
        .append_blocking(
            crate::journal::acp_envelope_record(session_id, 1, 1, &json!({"type":"pi_filler"}))
                .unwrap(),
        )
        .unwrap();
    journal
        .append_blocking(
            crate::journal::acp_envelope_record(
                session_id,
                1,
                2,
                &crate::claude_view::withheld_finish_marker(),
            )
            .unwrap(),
        )
        .unwrap();
    journal.start_generation(session_id, 2).unwrap();
    let turn_end = json!({
        "type": "turn_end",
        "message": {
            "role": "assistant",
            "content": [],
            "model": "pi-test",
            "usage": {"totalTokens": 7},
            "stopReason": "end_turn"
        },
        "toolResults": []
    });
    journal
        .append_blocking(crate::journal::acp_envelope_record(session_id, 2, 1, &turn_end).unwrap())
        .unwrap();

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    runtime.set_agent_kind(SessionKind::Pi);
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
        .expect("attach live pi session");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn);
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentFinished { .. }
        )),
        "the previous generation's stranded marker must not suppress the new generation's turn: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::ContextUsage { used_tokens: 7, .. })),
        "the new generation's turn_end derives in full: {events:?}"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}
