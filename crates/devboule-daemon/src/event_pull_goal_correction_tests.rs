//! The goal correction a short replay emits after its rows.

use super::super::*;
use super::*;

use super::test_support::drain;

/// The goal-correction fixtures: one journal, one runtime, one attachment.
/// `next_seq` above the last record's seq + 1 makes the replay short-page
/// (the attach watermark outruns the rows); equal makes it complete.
fn goal_correction_fixture(
    session_id: &str,
    next_seq: u64,
    records: Vec<crate::journal::EventRecord>,
) -> (
    std::path::PathBuf,
    Arc<Journal>,
    Arc<SessionRuntime>,
    Arc<ConnHandle>,
) {
    let dir = crate::test_dirs::test_temp_dir("devboule-replay-goal-correction");
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
    for record in records {
        journal.append_blocking(record).unwrap();
    }
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

fn goal_report_record(
    session_id: &str,
    seq: u64,
    goal: Option<&str>,
) -> crate::journal::EventRecord {
    crate::journal::EventRecord {
        session_id: session_id.to_string(),
        generation: 1,
        seq,
        kind: crate::journal::EventKind::AgentReport,
        ts_ms: 0,
        payload: serde_json::to_vec(&SessionEvent::GoalChanged {
            goal: goal.map(str::to_string),
        })
        .expect("the goal event serializes"),
    }
}

fn filler_report_record(session_id: &str, seq: u64) -> crate::journal::EventRecord {
    crate::journal::EventRecord {
        session_id: session_id.to_string(),
        generation: 1,
        seq,
        kind: crate::journal::EventKind::AgentReport,
        ts_ms: 0,
        payload: serde_json::to_vec(&SessionEvent::AgentError {
            message: format!("filler {seq}"),
        })
        .expect("the filler event serializes"),
    }
}

#[test]
fn goal_correction_follows_an_incomplete_replays_stale_goal() {
    let session_id = "s.replay.goal.correct";
    // next_seq 3 makes the watermark 2 while the rows stop at seq 1: the
    // short final page marks the replay incomplete.
    let (dir, journal, runtime, conn) = goal_correction_fixture(
        session_id,
        3,
        vec![goal_report_record(session_id, 1, Some("A"))],
    );
    runtime.set_goal(Some("B".to_string()));
    let events = drain(&conn);
    let goals: Vec<Option<String>> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::GoalChanged { goal } => Some(goal.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        goals,
        [Some("A".to_string()), Some("B".to_string())],
        "the replayed goal arrives, then the runtime goal corrects it — last"
    );
    assert!(events
        .iter()
        .any(|event| matches!(event, SessionEvent::JournalDegraded { .. })));

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn goal_correction_is_absent_after_a_complete_replay() {
    let session_id = "s.replay.goal.complete";
    let (dir, journal, runtime, conn) = goal_correction_fixture(
        session_id,
        2,
        vec![goal_report_record(session_id, 1, Some("A"))],
    );
    runtime.set_goal(Some("B".to_string()));
    let events = drain(&conn);
    assert!(events
        .iter()
        .all(|event| !matches!(event, SessionEvent::JournalDegraded { .. })));
    let goals: Vec<Option<String>> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::GoalChanged { goal } => Some(goal.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        goals,
        [Some("A".to_string())],
        "a complete replay speaks only from its rows"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn goal_correction_is_absent_on_the_live_degraded_path() {
    let session_id = "s.replay.goal.live-degraded";
    let (dir, journal, runtime, conn) = goal_correction_fixture(
        session_id,
        2,
        vec![goal_report_record(session_id, 1, Some("A"))],
    );
    runtime.set_goal(Some("B".to_string()));
    let replayed = drain(&conn);
    assert!(
        !replayed
            .iter()
            .any(|event| matches!(event, SessionEvent::JournalDegraded { .. })),
        "the replay itself must be complete for this test to mean anything"
    );
    runtime.mark_journal_degraded();
    let live = drain(&conn);
    assert!(live
        .iter()
        .any(|event| matches!(event, SessionEvent::JournalDegraded { .. })));
    assert!(
        !live
            .iter()
            .any(|event| matches!(event, SessionEvent::GoalChanged { .. })),
        "a live journal drop is not a replay hole; the goal stays with the live stream"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn goal_correction_clears_a_stale_goal_when_the_runtime_has_none() {
    let session_id = "s.replay.goal.null";
    let (dir, journal, runtime, conn) = goal_correction_fixture(
        session_id,
        3,
        vec![goal_report_record(session_id, 1, Some("A"))],
    );
    let events = drain(&conn);
    let goals: Vec<Option<String>> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::GoalChanged { goal } => Some(goal.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        goals,
        [Some("A".to_string()), None],
        "a runtime without a goal must clear the stale replayed one"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn goal_correction_writes_no_journal_row() {
    let session_id = "s.replay.goal.no-journal-row";
    let (dir, journal, runtime, conn) = goal_correction_fixture(
        session_id,
        3,
        vec![goal_report_record(session_id, 1, Some("A"))],
    );
    runtime.set_goal(Some("B".to_string()));
    let before = journal
        .replay(session_id)
        .expect("rows before")
        .events
        .len();
    let events = drain(&conn);
    // try_append is asynchronous; the flush rides the same FIFO queue the
    // appends rode, so the count below is settled rather than racing.
    journal.flush().expect("journal flush");
    let after = journal.replay(session_id).expect("rows after").events.len();
    assert_eq!(
        before, after,
        "the correction is a wire-only event; the journal must not grow"
    );
    assert!(events.iter().any(
        |event| matches!(&event, SessionEvent::GoalChanged { goal } if goal.as_deref() == Some("B"))
    ));

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn goal_correction_is_emitted_once_across_repeated_drains() {
    let session_id = "s.replay.goal.once";
    let (dir, journal, runtime, conn) = goal_correction_fixture(
        session_id,
        3,
        vec![goal_report_record(session_id, 1, Some("A"))],
    );
    runtime.set_goal(Some("B".to_string()));
    // Bounded rounds rather than `drain`: a pull path that re-emits the tail
    // every round never drains empty, and an unbounded loop would hang
    // instead of failing.
    let mut events = Vec::new();
    for _ in 0..3 {
        let batch = conn.pull_events();
        for event in &batch {
            conn.event_sent(event);
        }
        events.extend(batch.into_iter().map(|pending| pending.envelope.event));
    }
    let goals: Vec<Option<String>> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::GoalChanged { goal } => Some(goal.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        goals,
        [Some("A".to_string()), Some("B".to_string())],
        "one replayed goal, one correction — never a re-emission on a later round"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn goal_correction_overruns_when_the_tail_does_not_fit_the_batch() {
    let session_id = "s.replay.goal.full-batch";
    // Fourteen single-event rows plus the stale goal pad the replay so the
    // tail comes due with the batch one slot short of the pair: the tail
    // lands in this round, overrunning the per-round budget, instead of
    // waiting for a round that sibling attachments may keep filling first.
    let mut records = Vec::new();
    for seq in 1..=14 {
        records.push(filler_report_record(session_id, seq));
    }
    records.push(goal_report_record(session_id, 15, Some("A")));
    let (dir, journal, runtime, conn) = goal_correction_fixture(session_id, 17, records);
    runtime.set_goal(Some("B".to_string()));
    let batch = conn.pull_events();
    let events: Vec<SessionEvent> = batch
        .into_iter()
        .map(|pending| pending.envelope.event)
        .collect();
    assert_eq!(
        events.len(),
        PULL_BATCH + 1,
        "the padded replay leaves the pair one slot short and the tail lands in the same round"
    );
    assert!(
        matches!(&events[PULL_BATCH - 2], SessionEvent::GoalChanged { goal } if goal.as_deref() == Some("A")),
        "the stale replayed goal is the last replay event before the tail"
    );
    assert!(
        matches!(
            &events[PULL_BATCH - 1],
            SessionEvent::JournalDegraded { .. }
        ),
        "the notice precedes the correction"
    );
    assert!(
        matches!(&events[PULL_BATCH], SessionEvent::GoalChanged { goal } if goal.as_deref() == Some("B")),
        "the correction closes the saturated round"
    );
    let rest = drain(&conn);
    assert!(
        !rest.iter().any(|event| matches!(
            event,
            SessionEvent::GoalChanged { .. } | SessionEvent::JournalDegraded { .. }
        )),
        "the tail is not repeated once a round has carried it"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn goal_correction_is_emitted_into_a_saturated_round() {
    // The saturated turn of the stall shape, at unit scale: the shared
    // batch is already at the cap when this attachment's turn comes (a
    // sibling's earlier turn filled it), and the replay is past paging —
    // pending drained, manifest and plan usage delivered, the lag forced
    // the finish. The tail must ride this round, not wait for a round
    // whose budget a refilling sibling may never grant.
    let session_id = "s.replay.goal.saturated";
    let runtime = Arc::new(SessionRuntime::with_journal(session_id.to_string(), None));
    runtime.set_goal(Some("B".to_string()));
    let mut pull = PullState {
        runtime: Arc::clone(&runtime),
        attachment_key: AttachmentKey {
            conn_id: 1,
            subscription_id: 1,
        },
        transcript: false,
        transcript_cursor: None,
        agent_replay: Some(AgentReplay {
            from_seq: 0,
            cursor_generation: 1,
            cursor: 1,
            watermark: 2,
            generation: 1,
            pending: VecDeque::new(),
            replayed_seqs: std::collections::HashSet::new(),
            claude_view: None,
            codex_view: None,
            codex_plan_turns: None,
            marks_needed: false,
            is_pi: false,
            is_codex: false,
            manifest_emitted: true,
            plan_usage_delivered: true,
            catch_up_extensions: 0,
            durable_done: true,
            journal_lagged: true,
            force_finish: true,
        }),
        exit_sent: false,
        journal_degraded_sent: false,
        generation: 1,
        attachment_generation: 1,
    };
    let mut events: Vec<PendingEvent> = (0..PULL_BATCH)
        .map(|slot| {
            wire_event(
                session_id,
                &pull,
                1,
                SessionEvent::AgentError {
                    message: format!("sibling {slot}"),
                },
                None,
            )
        })
        .collect();
    pull_live_agent_replay_events(session_id, &mut pull, &mut events);
    assert_eq!(
        events.len(),
        PULL_BATCH + 2,
        "the two-event tail rides the saturated round instead of waiting for budget"
    );
    assert!(
        matches!(
            &events[PULL_BATCH].envelope.event,
            SessionEvent::JournalDegraded { .. }
        ),
        "the notice precedes the correction"
    );
    assert!(
        matches!(&events[PULL_BATCH + 1].envelope.event, SessionEvent::GoalChanged { goal } if goal.as_deref() == Some("B")),
        "the correction closes the saturated round"
    );
    assert!(
        pull.agent_replay.is_none(),
        "a round that carried the tail ended the replay"
    );
}

#[test]
fn goal_correction_precedes_a_live_goal_queued_behind_the_replay() {
    let session_id = "s.replay.goal.live-behind";
    let (dir, journal, runtime, conn) = goal_correction_fixture(
        session_id,
        3,
        vec![goal_report_record(session_id, 1, Some("A"))],
    );
    runtime.set_goal(Some("B".to_string()));
    // Queued while the replay is still unread; journal_text None keeps it
    // out of the journal so the replay itself cannot re-derive it.
    runtime.publish_agent_event(
        SessionEvent::GoalChanged {
            goal: Some("C".to_string()),
        },
        None,
    );
    let events = drain(&conn);
    let goals: Vec<Option<&str>> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::GoalChanged { goal } => Some(goal.as_deref()),
            _ => None,
        })
        .collect();
    assert_eq!(
        goals,
        [Some("A"), Some("B"), Some("C")],
        "replay, then correction, then the live goal — each exactly once, in stream order"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}
