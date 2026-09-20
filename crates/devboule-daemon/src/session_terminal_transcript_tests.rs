//! The terminal-attach, flood and transcript tests, moved whole out of
//! `session_tests.rs` lines 1065-1871 (at `4b8bb76`): an attach whose snapshot
//! and live stream meet at an exact boundary, a flood that never duplicates or
//! skips a frame and leaves a reattached client equal to a fresh emulator, the
//! pending queue's byte and frame budget, a DSR reply written straight to the
//! PTY, a control path that stays responsive under flood, two observers
//! receiving one stream while a detach clears only its own connection, a
//! permission card reaching a late observer once, the last transcript detach
//! removing the idle registry entry, the transcript store holding the whole
//! history whatever the cursor says, a stale generation rejected, the journal
//! keeping drain bytes after a reap, the coalesce constants small enough for an
//! echo, and a PTY error exposing only the OS code. Every line below is
//! byte-identical to its text there apart from this header; `sink_runtime`,
//! `apply_snapshot_state` and `flood_chunk` are promoted to `pub(super)` for
//! this move, and the other fixtures come from the provider's own imports.

use super::tests::{
    apply_snapshot_state, attach_tracked, drain, ended_record, flood_chunk, insert_live,
    insert_transcript, permission_attention_event, sink_runtime, test_owner, tmp_delete_registry,
};
use super::*;

#[test]
fn attach_delivers_snapshot_then_live_with_exact_boundary() {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.publish_output("before");
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);
    assert_eq!(runtime.last_applied_seq(), 1);

    let events = drain(&conn);
    let [SessionEvent::Snapshot {
        as_of_seq, data, ..
    }] = &events[..]
    else {
        panic!("expected a single snapshot, got {events:?}");
    };
    assert_eq!(*as_of_seq, 1);
    assert!(data.contains("before"), "snapshot data: {data:?}");

    runtime.publish_output("after");
    let events = drain(&conn);
    assert_eq!(
        events,
        vec![SessionEvent::Output {
            seq: 2,
            data: "after".to_string()
        }]
    );
}

#[test]
fn attach_during_flood_never_duplicates_or_skips() {
    let runtime = Arc::new(SessionRuntime::new());
    let flood_runtime = Arc::clone(&runtime);
    let flood = std::thread::Builder::new()
        .name("flood".into())
        .spawn(move || {
            for index in 1..=4_000 {
                flood_runtime.publish_output(&flood_chunk(index));
                if index % 32 == 0 {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        })
        .expect("flood thread");

    let mut seen = std::collections::HashSet::new();
    let mut covered_to = 0u64;
    for epoch in 0u64..25 {
        let conn = ConnHandle::new(epoch + 1);
        attach_tracked(&runtime, &conn);
        let events = drain(&conn);
        assert!(
            !events.is_empty(),
            "epoch {epoch} saw nothing: attach must enqueue a snapshot"
        );
        // Spread the attach epochs across the flood's lifetime.
        std::thread::sleep(Duration::from_millis(4));
        let mut expected = None;
        for event in &events {
            match event {
                SessionEvent::Snapshot { as_of_seq, .. } => {
                    assert!(
                        *as_of_seq >= covered_to,
                        "snapshot boundary moved backwards at epoch {epoch}"
                    );
                    covered_to = (*as_of_seq).max(covered_to);
                    expected = Some(as_of_seq + 1);
                }
                SessionEvent::Output { seq, .. } => {
                    if let Some(expected_seq) = expected {
                        assert_eq!(
                            *seq, expected_seq,
                            "output skipped or duplicated at epoch {epoch}"
                        );
                    }
                    expected = Some(seq + 1);
                    assert!(seen.insert(*seq), "sequence {seq} delivered twice");
                    covered_to = (*seq).max(covered_to);
                }
                SessionEvent::Exit { .. } => {}
                other => panic!("unexpected event at epoch {epoch}: {other:?}"),
            }
        }
        runtime.detach_if_conn(conn.id);
    }
    flood.join().expect("flood thread joins");

    // The flood is complete: one final attach must now deliver (or
    // subsume) everything it published.
    let conn = ConnHandle::new(999);
    attach_tracked(&runtime, &conn);
    for event in drain(&conn) {
        match event {
            SessionEvent::Snapshot { as_of_seq, .. } => covered_to = as_of_seq.max(covered_to),
            SessionEvent::Output { seq, .. } => {
                assert!(seen.insert(seq), "sequence {seq} delivered twice");
                covered_to = seq.max(covered_to);
            }
            _ => {}
        }
    }
    assert_eq!(
        covered_to, 4_000,
        "the flood was not fully delivered or subsumed"
    );
}

#[test]
fn reattach_mid_flood_screen_equals_a_fresh_emulator() {
    let runtime = Arc::new(SessionRuntime::new());
    let mut reference = Screen::new(INITIAL_COLS, INITIAL_ROWS);

    fn apply(screen: &mut Screen, event: &SessionEvent) {
        match event {
            SessionEvent::Snapshot { .. } => apply_snapshot_state(screen, event),
            SessionEvent::Output { data, .. } => screen.process(data.as_bytes()),
            _ => {}
        }
    }

    // Phase 1: publish while detached, then attach and synchronise.
    for index in 1..=60 {
        let chunk = flood_chunk(index);
        runtime.publish_output(&chunk);
        reference.process(chunk.as_bytes());
    }
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);
    let mut client = Screen::new(INITIAL_COLS, INITIAL_ROWS);
    for event in drain(&conn) {
        apply(&mut client, &event);
    }
    assert_eq!(
        client.snapshot(),
        reference.snapshot(),
        "snapshot state must equal the emulator after phase 1"
    );

    // Phase 2: live chunks while attached, then reattach from scratch.
    for index in 61..=120 {
        let chunk = flood_chunk(index);
        runtime.publish_output(&chunk);
        reference.process(chunk.as_bytes());
    }
    for event in drain(&conn) {
        apply(&mut client, &event);
    }
    assert_eq!(client.snapshot(), reference.snapshot());

    runtime.detach_if_conn(conn.id);
    let conn = ConnHandle::new(2);
    attach_tracked(&runtime, &conn);
    let mut client = Screen::new(INITIAL_COLS, INITIAL_ROWS);
    for event in drain(&conn) {
        apply(&mut client, &event);
    }
    assert_eq!(
        client.snapshot(),
        reference.snapshot(),
        "snapshot + subsequent events must equal a fresh emulator fed the whole stream"
    );
}

#[test]
fn slow_client_is_resynchronised_with_a_snapshot() {
    let runtime = Arc::new(SessionRuntime::new());
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);

    // Stop reading: publish well past the frame budget without pulling.
    for index in 1..=200 {
        runtime.publish_output(&format!("slow-{index:04}\r\n"));
    }

    let events = drain(&conn);
    let mut expected = None;
    let mut client = Screen::new(INITIAL_COLS, INITIAL_ROWS);
    let mut reference = Screen::new(INITIAL_COLS, INITIAL_ROWS);
    let mut seen = std::collections::HashSet::new();
    for event in &events {
        match event {
            SessionEvent::Snapshot { as_of_seq, .. } => {
                expected = Some(as_of_seq + 1);
                apply_snapshot_state(&mut client, event);
            }
            SessionEvent::Output { seq, data } => {
                assert_eq!(*seq, expected.expect("outputs follow the snapshot"));
                expected = Some(seq + 1);
                assert!(seen.insert(*seq), "sequence {seq} delivered twice");
                client.process(data.as_bytes());
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
    for index in 1..=200 {
        reference.process(format!("slow-{index:04}\r\n").as_bytes());
    }
    assert_eq!(
        client.snapshot(),
        reference.snapshot(),
        "the resynchronised screen must still be the true screen"
    );
}

#[test]
fn pending_queue_never_exceeds_byte_or_frame_budget() {
    let runtime = Arc::new(SessionRuntime::new());
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);
    let payload = "x".repeat(COALESCE_MAX_BYTES);

    for _ in 0..200 {
        runtime.publish_output(&payload);
        let stream = runtime.stream.lock().expect("stream lock");
        assert!(stream
            .observers
            .values()
            .all(|attachment| attachment.pending_bytes <= PENDING_OUTPUT_BUDGET_BYTES));
        assert!(stream
            .observers
            .values()
            .all(|attachment| attachment.pending_frames <= PENDING_OUTPUT_BUDGET_FRAMES));
    }
}

#[test]
fn dsr_reply_is_written_straight_to_the_pty() {
    let (runtime, received) = sink_runtime();
    // No attachment, no journal, no snapshot: the query is answered on
    // the publish path itself.
    runtime.publish_output("\x1b[2;3H\x1b[6n");
    assert_eq!(
        String::from_utf8(received.lock().unwrap().clone()).expect("utf8"),
        "\x1b[2;3R",
        "one one-based CPR reply, routed to the PTY writer"
    );
    runtime.publish_output("plain");
    assert_eq!(received.lock().unwrap().len(), 6, "no extra replies");
}

#[test]
fn control_path_stays_responsive_under_flood() {
    let runtime = Arc::new(SessionRuntime::new());
    let flood_runtime = Arc::clone(&runtime);
    let stop = Arc::new(AtomicBool::new(false));
    let flood_stop = Arc::clone(&stop);
    let flood = std::thread::Builder::new()
        .name("flood".into())
        .spawn(move || {
            let chunk = "x".repeat(COALESCE_MAX_BYTES);
            while !flood_stop.load(Ordering::Acquire) {
                for _ in 0..16 {
                    flood_runtime.publish_output(&chunk);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        })
        .expect("flood thread");

    let mut worst = Duration::ZERO;
    for epoch in 0..200u64 {
        let started = Instant::now();
        let conn = ConnHandle::new(epoch + 1);
        runtime
            .try_attach_with_replay(None, &conn, false)
            .expect("attach under flood");
        runtime.detach_if_conn(conn.id);
        worst = worst.max(started.elapsed());
    }
    stop.store(true, Ordering::Release);
    flood.join().expect("flood thread joins");
    // Screen capture + registration is two grid copies under the lock;
    // if the publish path ever held the mutex across slow work, this
    // would blow far past the bound. 1 s is orders of magnitude above
    // the observed cost and 30x below the RPC timeout this milestone
    // exists to fix.
    assert!(
        worst < Duration::from_secs(1),
        "state lock starved under flood: {worst:?}"
    );
}

#[test]
fn two_observers_receive_the_same_output() {
    let runtime = Arc::new(SessionRuntime::new());
    let first = ConnHandle::new(1);
    let second = ConnHandle::new(2);
    let first_outcome = runtime
        .try_attach_with_subscription(101, None, &first, false)
        .expect("first observer");
    first
        .track_with_subscription(
            101,
            Arc::clone(&runtime),
            false,
            None,
            first_outcome.generation,
            first_outcome.live_agent_replay,
        )
        .expect("first subscription");
    let second_outcome = runtime
        .try_attach_with_subscription(202, None, &second, false)
        .expect("second observer");
    second
        .track_with_subscription(
            202,
            Arc::clone(&runtime),
            false,
            None,
            second_outcome.generation,
            second_outcome.live_agent_replay,
        )
        .expect("second subscription");
    runtime
        .claim_resize(first.id, 101)
        .expect("first observer claims resize control");
    let _ = drain(&first);
    let _ = drain(&second);

    runtime.publish_output("shared");

    assert_eq!(
        drain(&first),
        vec![SessionEvent::Output {
            seq: 1,
            data: "shared".to_string(),
        }]
    );
    assert_eq!(
        drain(&second),
        vec![SessionEvent::Output {
            seq: 1,
            data: "shared".to_string(),
        }]
    );
    assert_eq!(runtime.resize_owner_conn_id(), Some(1));
}

#[test]
fn same_connection_can_reattach() {
    let runtime = SessionRuntime::new();
    let conn = ConnHandle::new(7);
    runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("first");
    runtime
        .try_attach_with_replay(
            Some(Cursor {
                generation: 1,
                seq: 0,
            }),
            &conn,
            false,
        )
        .expect("reattach");
    assert_eq!(runtime.resize_owner_conn_id(), Some(7));
}

#[test]
fn detaching_one_observer_leaves_the_other_live() {
    let runtime = Arc::new(SessionRuntime::new());
    let first = ConnHandle::new(3);
    let second = ConnHandle::new(4);
    let first_outcome = runtime
        .try_attach_with_subscription(301, None, &first, false)
        .expect("first observer");
    first
        .track_with_subscription(
            301,
            Arc::clone(&runtime),
            false,
            None,
            first_outcome.generation,
            first_outcome.live_agent_replay,
        )
        .expect("first subscription");
    let second_outcome = runtime
        .try_attach_with_subscription(402, None, &second, false)
        .expect("second observer");
    second
        .track_with_subscription(
            402,
            Arc::clone(&runtime),
            false,
            None,
            second_outcome.generation,
            second_outcome.live_agent_replay,
        )
        .expect("second subscription");
    let _ = drain(&first);
    let _ = drain(&second);

    runtime.detach_subscription(first.id, 301);
    first.untrack_subscription(301);
    runtime.publish_output("still-live");

    assert!(drain(&first).is_empty());
    assert_eq!(
        drain(&second),
        vec![SessionEvent::Output {
            seq: 1,
            data: "still-live".to_string(),
        }]
    );
}

#[test]
fn typed_permission_request_reaches_a_late_observer() {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.stream.lock().unwrap().screen = None;
    let first = ConnHandle::new(5);
    let first_outcome = runtime
        .try_attach_with_subscription(501, None, &first, true)
        .expect("first observer");
    first
        .track_with_subscription(
            501,
            Arc::clone(&runtime),
            false,
            None,
            first_outcome.generation,
            first_outcome.live_agent_replay,
        )
        .expect("first subscription");

    runtime.publish_agent_event(permission_attention_event(), None);
    let first_events = first.pull_events();
    assert!(first_events.iter().any(|event| matches!(
        event.envelope.event,
        SessionEvent::PermissionRequest { ref tool_call_id, .. } if tool_call_id == "tool-attention"
    )));
    for event in &first_events {
        first.event_sent(event);
    }

    let second = ConnHandle::new(6);
    let second_outcome = runtime
        .try_attach_with_subscription(602, None, &second, true)
        .expect("late observer");
    second
        .track_with_subscription(
            602,
            Arc::clone(&runtime),
            false,
            None,
            second_outcome.generation,
            second_outcome.live_agent_replay,
        )
        .expect("second subscription");
    let second_events = second.pull_events();
    assert!(second_events.iter().any(|event| matches!(
        event.envelope.event,
        SessionEvent::PermissionRequest { ref tool_call_id, .. } if tool_call_id == "tool-attention"
    )));
}

#[test]
fn detached_permission_request_reaches_a_late_observer_once() {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.stream.lock().unwrap().screen = None;
    let first = ConnHandle::new(7);
    runtime
        .try_attach_with_subscription(701, None, &first, true)
        .expect("first observer");

    runtime.publish_agent_event(permission_attention_event(), None);
    runtime.detach_subscription(first.id, 701);

    let second = ConnHandle::new(8);
    let second_outcome = runtime
        .try_attach_with_subscription(802, None, &second, true)
        .expect("late observer");
    second
        .track_with_subscription(
            802,
            Arc::clone(&runtime),
            false,
            None,
            second_outcome.generation,
            second_outcome.live_agent_replay,
        )
        .expect("second subscription");
    let events = second.pull_events();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.envelope.event,
                SessionEvent::PermissionRequest { ref tool_call_id, .. }
                    if tool_call_id == "tool-attention"
            ))
            .count(),
        1
    );
}

#[test]
fn last_detach_keeps_runtime_and_allows_later_attach() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-last-detach", "process-last-detach");
    let session_id = "s.last-detach.1";
    // `insert_live` seats the runtime but writes no `sessions` row, so every
    // frame this session appended failed ("No session with that id.") and only
    // the observation of the failure raced the writer. The row is what makes
    // the assertion below about delivery instead of about that race.
    journal
        .upsert_blocking(crate::journal::new_session_record(
            session_id,
            owner.user.clone(),
            None,
            SessionKind::Terminal,
            "Terminal",
        ))
        .expect("journal row");
    insert_live(&registry, session_id, owner.clone());
    let runtime = registry.runtime(session_id).expect("runtime");
    let first = ConnHandle::new(5);
    registry
        .attach_with_subscription(session_id, 501, None, &first, &owner, false)
        .expect("first observer");
    let _ = drain(&first);

    registry
        .detach_with_subscription(session_id, 501, &first, &owner)
        .expect("first observer detaches");
    assert!(!runtime.process_exited());
    assert!(registry.runtime(session_id).is_ok());
    assert!(runtime.stream.lock().expect("stream").observers.is_empty());

    let third = ConnHandle::new(6);
    registry
        .attach_with_subscription(session_id, 603, None, &third, &owner, false)
        .expect("later observer");
    let _ = drain(&third);
    runtime.publish_output("after-detach");

    assert_eq!(
        drain(&third),
        vec![SessionEvent::Output {
            seq: 1,
            data: "after-detach".to_string(),
        }],
        "the frame reached a durable session: no degradation frame follows it"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn last_transcript_detach_removes_idle_registry_entry() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-transcript-idle", "process-transcript-idle");
    let session_id = "s.transcript-idle.1";
    journal
        .upsert_blocking(ended_record(session_id, &owner.user))
        .expect("journal row");
    insert_transcript(&registry, session_id, owner.clone());

    let conn = ConnHandle::new(7);
    registry
        .attach_with_subscription(session_id, 701, None, &conn, &owner, false)
        .expect("transcript observer attaches");
    registry
        .detach_with_subscription(session_id, 701, &conn, &owner)
        .expect("transcript observer detaches");

    assert!(registry.runtime(session_id).is_err());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn delivered_transcript_exit_removes_the_idle_registry_entry() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-transcript-exit", "process-transcript-exit");
    let session_id = "s.transcript-exit.1";
    journal
        .upsert_blocking(ended_record(session_id, &owner.user))
        .expect("journal row");
    insert_transcript(&registry, session_id, owner.clone());

    let conn = ConnHandle::new(8);
    registry
        .attach_with_subscription(session_id, 801, None, &conn, &owner, false)
        .expect("transcript observer attaches");
    let events = conn.pull_events();
    assert!(events
        .iter()
        .any(|event| matches!(event.envelope.event, SessionEvent::Exit { .. })));
    for event in &events {
        conn.event_sent(event);
    }
    registry.subscription_event_sent(session_id);

    assert!(registry.runtime(session_id).is_err());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The transcript store holds the whole history whatever the cursor says.
/// A reattaching reader presents a cursor that is a position inside the
/// current generation only — history does not advance cursors, so a cursor
/// can never certify the history was read — and the hydration behind the
/// store must therefore read unpositioned: the pull, through the owed-row
/// predicate, decides what is delivered.
#[test]
fn the_transcript_store_holds_the_whole_history_whatever_the_cursor_says() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-1", "probe");
    let session_id = "s.store.all.1";
    journal
        .upsert_blocking(crate::journal::new_session_record(
            session_id,
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("journal row");
    let ledger_row = crate::journal::output_record(session_id, 1, 1, "gen-1 ledger".as_bytes());
    journal.append_blocking(ledger_row).expect("gen-1 ledger");
    let user_row = crate::journal::agent_report_record(
        session_id,
        1,
        2,
        &SessionEvent::AgentUserMessage {
            message_id: Some("m1".into()),
            text: "gen-1 user".into(),
            author: devboule_protocol::UserMessageAuthor::Human,
            message_kind: devboule_protocol::UserMessageKind::Unknown,
        },
    )
    .unwrap();
    journal.append_blocking(user_row).expect("gen-1 user row");
    journal.start_generation(session_id, 2).expect("resume");
    let ledger_after = crate::journal::output_record(session_id, 2, 6, "gen-2 ledger".as_bytes());
    journal.append_blocking(ledger_after).expect("gen-2 ledger");
    let answer_row = crate::journal::agent_report_record(
        session_id,
        2,
        7,
        &SessionEvent::AgentMessage {
            message_id: Some("m2".into()),
            text: "after cursor".into(),
            parent_tool_use_id: None,
            spawn_depth: None,
        },
    )
    .unwrap();
    journal.append_blocking(answer_row).expect("gen-2 answer");

    // A reattaching reader whose cursor says it is at seq 5 of generation 2.
    let conn = ConnHandle::new(9);
    registry
        .attach_with_subscription(
            session_id,
            901,
            Some(Cursor {
                generation: 2,
                seq: 5,
            }),
            &conn,
            &owner,
            false,
        )
        .expect("transcript observer attaches");
    let mut events = conn.pull_events();
    for event in &events {
        conn.event_sent(event);
    }
    loop {
        let more = conn.pull_events();
        if more.is_empty() {
            break;
        }
        for event in &more {
            conn.event_sent(event);
        }
        events.extend(more);
    }
    let transcript: Vec<String> = events
        .iter()
        .filter_map(|event| match &event.envelope.event {
            SessionEvent::Output { data, .. } => Some(data.clone()),
            SessionEvent::AgentUserMessage { text, .. } => Some(text.clone()),
            SessionEvent::AgentMessage { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        transcript,
        vec!["gen-1 ledger", "gen-1 user", "gen-2 ledger", "after cursor",],
        "the store must hold the whole history whatever the cursor says: {transcript:?}"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn stale_generation_is_rejected() {
    let runtime = SessionRuntime::new();
    runtime.bump_generation();
    let conn = ConnHandle::new(1);
    let err = runtime
        .try_attach_with_replay(
            Some(Cursor {
                generation: 1,
                seq: 0,
            }),
            &conn,
            false,
        )
        .err()
        .expect("stale generation must be rejected");
    assert_eq!(err.code, ErrorCode::SessionGenerationMismatch);
}

#[test]
fn detach_clears_only_this_connection() {
    let runtime = SessionRuntime::new();
    let conn = ConnHandle::new(3);
    runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    runtime.detach_if_conn(3);
    assert_eq!(runtime.resize_owner_conn_id(), None);
}

#[test]
fn journal_keeps_drain_bytes_after_reap() {
    let dir = std::env::temp_dir().join(format!(
        "devboule-drain-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    journal
        .upsert_blocking(new_session_record(
            "s.drain.1",
            "S-1-5-21-1",
            None,
            SessionKind::Terminal,
            "Terminal",
        ))
        .unwrap();
    let runtime = Arc::new(SessionRuntime::with_journal(
        "s.drain.1".into(),
        Some(Arc::clone(&journal)),
    ));
    runtime.publish_output("HEAD");
    journal.flush().unwrap();
    runtime.mark_exited(Some(0));
    journal.flush().unwrap();
    let tail = "X".repeat(3953);
    runtime.publish_output(&tail);
    journal.flush().unwrap();
    runtime.close_output();
    journal.try_mark_ended("s.drain.1", 1, Some(0));
    journal.flush().unwrap();
    assert_eq!(runtime.published_frames.load(Ordering::Relaxed), 2);
    let stats = journal.stats();
    assert_eq!(stats.accepted_frames, 2);
    assert_eq!(stats.committed_frames, 2);
    assert_eq!(stats.failed_frames, 0);
    let replay = journal.replay("s.drain.1").unwrap();
    let replay_bytes: usize = replay
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { data, .. } => Some(data.len()),
            _ => None,
        })
        .sum();
    assert_eq!(replay_bytes, 4 + 3953, "journal silently lost drain bytes");
    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn coalesce_constants_are_small_enough_for_echo() {
    const {
        assert!(COALESCE_MAX_BYTES <= 16 * 1024);
    }
    const {
        assert!(COALESCE_MAX_BYTES >= 1024);
    }
    assert!(COALESCE_FLUSH <= Duration::from_millis(16));
}

#[test]
fn pty_error_exposes_only_the_os_code_to_clients() {
    let detail = "CreateProcessW command=C:\\Users\\secret\\shell.exe (os error 1450)";
    let code = extract_os_error_code(detail).expect("OS error code");
    assert_eq!(code, 1450);
    assert_eq!(os_error_description(code), "no system resources");
    let wire = pty_wire_error("Could not start the terminal shell.", detail);
    assert_eq!(
        wire.message,
        "Could not start the terminal shell. (OS error 1450: no system resources)."
    );
    assert!(!wire.message.contains("secret"));
}
