//! What a live attach delivers, and what a recovered pull ends with.

use super::super::*;
use super::*;

use super::test_support::{attach_tracked, drain};

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
            SessionEvent::QueueSnapshot { .. } => "queue_snapshot",
            SessionEvent::TasksSnapshot { .. } => "tasks_snapshot",
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
            SessionEvent::AgentTasks { .. } => "agent_tasks",
            SessionEvent::GoalChanged { .. } => "goal_changed",
            SessionEvent::AgentError { .. } => "agent_error",
            SessionEvent::AgentStderr { .. } => "agent_stderr",
            SessionEvent::PermissionRequest { .. } => "permission_request",
            SessionEvent::PermissionResolved { .. } => "permission_resolved",
            SessionEvent::PermissionAnswered { .. } => "permission_answered",
            SessionEvent::SessionManifest { .. } => "session_manifest",
            SessionEvent::SessionFeatureState { .. } => "session_feature_state",
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
    // The durable boundary drains every queued frame, however long a loaded
    // runner takes; the outer bound only keeps a stuck writer a failed test
    // instead of a hung CI job.
    let (finished, waiting) = std::sync::mpsc::channel();
    let draining = Arc::clone(&journal);
    std::thread::spawn(move || {
        let _ = finished.send(draining.mark_ended_blocking("s.recover.1", 1, Some(0)));
    });
    match waiting.recv_timeout(std::time::Duration::from_secs(120)) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("the durable end failed: {error}"),
        Err(_) => panic!("the durable end did not drain within 120 s; the writer is stuck"),
    }

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
        event_ts_ms: vec![None; 2],
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
            SessionEvent::QueueSnapshot { .. } => "queue_snapshot",
            SessionEvent::TasksSnapshot { .. } => "tasks_snapshot",
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
            SessionEvent::AgentTasks { .. } => "agent_tasks",
            SessionEvent::GoalChanged { .. } => "goal_changed",
            SessionEvent::AgentError { .. } => "agent_error",
            SessionEvent::AgentStderr { .. } => "agent_stderr",
            SessionEvent::PermissionRequest { .. } => "permission_request",
            SessionEvent::PermissionResolved { .. } => "permission_resolved",
            SessionEvent::PermissionAnswered { .. } => "permission_answered",
            SessionEvent::SessionManifest { .. } => "session_manifest",
            SessionEvent::SessionFeatureState { .. } => "session_feature_state",
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
