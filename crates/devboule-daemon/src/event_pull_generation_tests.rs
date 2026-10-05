//! What a replayed row carries: its own generation and turn time.

use super::super::*;
use super::*;

use super::test_support::drain;

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
        at_ms: None,
        images: Vec::new(),
    };
    let answer_before = SessionEvent::AgentMessage {
        message_id: Some("m2".into()),
        text: "gen-1 answer".into(),
        parent_tool_use_id: None,
        spawn_depth: None,

        images: Vec::new(),
    };
    let answer_after = SessionEvent::AgentMessage {
        message_id: Some("m3".into()),
        text: "after resume".into(),
        parent_tool_use_id: None,
        spawn_depth: None,

        images: Vec::new(),
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

/// A replayed user message takes its time from the journal row's own
/// `ts_ms` column — the same clock the live event was set from — so a row
/// whose payload predates `at_ms` still gives its turn a time.
#[test]
fn replay_stamps_the_user_turn_time_from_the_journal_row() {
    let dir = crate::test_dirs::test_temp_dir("devboule-replay-turn-time");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.replay.turn.time";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    let untimed = SessionEvent::AgentUserMessage {
        message_id: Some("m1".into()),
        text: "written before the turn time existed".into(),
        author: devboule_protocol::UserMessageAuthor::Human,
        message_kind: devboule_protocol::UserMessageKind::Composer,
        at_ms: None,
        images: Vec::new(),
    };
    let row_ts = 1_789_053_471_559_u64;
    journal
        .append_blocking(crate::journal::EventRecord {
            session_id: session_id.to_string(),
            generation: 1,
            seq: 1,
            kind: crate::journal::EventKind::AgentReport,
            ts_ms: row_ts,
            payload: serde_json::to_vec(&untimed).unwrap(),
        })
        .unwrap();

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.generation = 1;
        stream.next_seq = 2;
    }
    runtime.generation.store(1, Ordering::Release);
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
    let at = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentUserMessage { at_ms, .. } => Some(*at_ms),
            _ => None,
        })
        .expect("the user message is replayed");
    assert_eq!(at, Some(row_ts), "the row's own time is the turn time");

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A kind-less native row — one the daemon itself wrote before
/// `message_kind` existed — decodes as `Unknown` and still takes its turn
/// time from its journal row: the stamp reads the row source, and the app's
/// legacy classifier renders such a row as a composer bubble.
#[test]
fn replay_times_a_pre_message_kind_row_from_its_row() {
    let dir = crate::test_dirs::test_temp_dir("devboule-replay-legacy-turn-time");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.replay.legacy.turn.time";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    // The payload as the pre-`message_kind` writer stored it: no
    // `messageKind`, no `atMs`.
    let legacy = serde_json::json!({
        "type": "agent_user_message",
        "messageId": "m0",
        "text": "written before message_kind existed",
        "author": "human"
    });
    let row_ts = 1_789_053_471_559_u64;
    journal
        .append_blocking(crate::journal::EventRecord {
            session_id: session_id.to_string(),
            generation: 1,
            seq: 1,
            kind: crate::journal::EventKind::AgentReport,
            ts_ms: row_ts,
            payload: serde_json::to_vec(&legacy).unwrap(),
        })
        .unwrap();

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.generation = 1;
        stream.next_seq = 2;
    }
    runtime.generation.store(1, Ordering::Release);
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
    let at = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentUserMessage { at_ms, .. } => Some(*at_ms),
            _ => None,
        })
        .expect("the legacy user message is replayed");
    assert_eq!(
        at,
        Some(row_ts),
        "a pre-message_kind turn is timed from its row, like a composer row"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A historical `user_message_chunk` envelope row may derive an `Unknown`
/// user-message view, but it receives no turn time, though its row carries a
/// `ts_ms` the stamp could read. The live ACP client returns before
/// journaling or publishing `user_message_chunk`; only rows written before
/// that guard replay through this converter.
#[test]
fn replay_gives_an_acp_echo_no_turn_time() {
    let dir = crate::test_dirs::test_temp_dir("devboule-replay-acp-echo-time");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.replay.acp.echo.time";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    let echo = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": "acp-1", "update": {
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "an acp echo of the prompt"}
        }}
    });
    journal
        .append_blocking(
            crate::journal::acp_envelope_record(session_id, 1, 1, &echo)
                .expect("the envelope serializes"),
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
        stream.generation = 1;
        stream.next_seq = 2;
    }
    runtime.generation.store(1, Ordering::Release);
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
    let (at, kind) = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentUserMessage {
                at_ms,
                message_kind,
                ..
            } => Some((*at_ms, *message_kind)),
            _ => None,
        })
        .expect("the echo is replayed");
    assert_eq!(
        kind,
        devboule_protocol::UserMessageKind::Unknown,
        "a historical provider-envelope echo derives as `Unknown`"
    );
    assert_eq!(
        at, None,
        "a historical provider-envelope echo remains unstamped"
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
        at_ms: None,
        images: Vec::new(),
    };
    let answer_after = SessionEvent::AgentMessage {
        message_id: Some("m2".into()),
        text: "gen-2 answer".into(),
        parent_tool_use_id: None,
        spawn_depth: None,

        images: Vec::new(),
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
