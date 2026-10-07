//! Tests for the session runtime: turn tokens, attention state and the agent snapshot.

use super::super::event_pull::ConnHandle;
use super::*;
use crate::journal::{PersistStatus, SessionRecord};
use crate::journal_lookback::{LookbackAnswer, LookbackRequest};
use devboule_protocol::{NoticeSeverity, SessionModeStateView, SessionModeView};

fn pull_events(conn: &ConnHandle) -> Vec<SessionEvent> {
    conn.pull_events()
        .into_iter()
        .map(|pending| pending.envelope.event)
        .collect()
}

#[test]
fn entering_plan_remembers_the_mode_to_resume() {
    let runtime = SessionRuntime::new();
    for mode in ["bypassPermissions", "plan"] {
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("claude".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(SessionModeStateView {
                current_mode_id: mode.to_string(),
                available_modes: Vec::new(),
            }),
        });
        runtime
            .record_claude_mode_report(mode)
            .expect("record reported mode");
    }
    assert_eq!(
        runtime.mode_before_plan_id().as_deref(),
        Some("bypassPermissions")
    );
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: Some("claude".to_string()),
        current_model_id: None,
        models: Vec::new(),
        modes: Some(SessionModeStateView {
            current_mode_id: "acceptEdits".to_string(),
            available_modes: Vec::new(),
        }),
    });
    runtime
        .record_claude_mode_report("acceptEdits")
        .expect("record reported mode");
    assert_eq!(runtime.mode_before_plan_id(), None);
}

/// The takeover tells every *other* observer that its view is dead. That
/// must travel as the event's identity, not as a sentence on AgentError —
/// the event a single malformed output line rides on — which the app can
/// only tell apart by comparing English.
#[test]
fn a_replaced_generation_tells_the_other_observer_by_event_identity() {
    let runtime = SessionRuntime::new();
    let taker = ConnHandle::new(1);
    runtime
        .try_attach_with_replay(None, &taker, false)
        .expect("the taking-over client attaches");
    let observer = ConnHandle::new(2);
    runtime
        .try_attach_with_replay(None, &observer, false)
        .expect("the replaced observer attaches");

    runtime.notify_generation_replaced(taker.id);

    let told = |conn: &ConnHandle| {
        conn.outbound
            .pull_replies()
            .into_iter()
            .filter_map(|reply| match reply {
                devboule_protocol::DaemonMessage::SubscriptionEvent { envelope, .. } => {
                    Some(envelope.event)
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert!(
        told(&taker).is_empty(),
        "the connection that took the session over is not the one told"
    );
    let events = told(&observer);
    assert_eq!(events.len(), 1, "the replaced view learns exactly once");
    assert!(
        matches!(events[0], SessionEvent::Detached),
        "the replaced view must be named by the event itself, not by prose on AgentError: {:?}",
        events[0]
    );
}

fn model(model_id: &str) -> SessionModel {
    SessionModel {
        model_id: model_id.to_string(),
        name: model_id.to_string(),
        description: None,
        context_tokens: None,
        current_effort: None,
        efforts: None,
    }
}

#[test]
fn store_session_manifest_preserves_thin_updates() {
    let runtime = SessionRuntime::new();
    let modes = SessionModeStateView {
        current_mode_id: "ask".to_string(),
        available_modes: vec![SessionModeView {
            id: "ask".to_string(),
            name: "Ask".to_string(),
            description: None,
        }],
    };
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: Some("grok".to_string()),
        current_model_id: Some("grok-4.6".to_string()),
        models: vec![model("grok-4.4"), model("grok-4.5"), model("grok-4.6")],
        modes: Some(modes.clone()),
    });

    let returned = runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: Some("grok".to_string()),
        current_model_id: None,
        models: Vec::new(),
        modes: None,
    });
    let SessionEvent::SessionManifest {
        current_model_id,
        models,
        modes: returned_modes,
        ..
    } = &returned
    else {
        panic!("expected session manifest");
    };
    assert_eq!(current_model_id.as_deref(), Some("grok-4.6"));
    assert_eq!(
        models
            .iter()
            .map(|model| model.model_id.as_str())
            .collect::<Vec<_>>(),
        ["grok-4.4", "grok-4.5", "grok-4.6"]
    );
    assert_eq!(returned_modes.as_ref(), Some(&modes));
    assert_eq!(runtime.session_manifest(), Some(returned));
}

#[test]
fn mode_update_surfaces_a_poisoned_manifest_lock() {
    let runtime = SessionRuntime::new();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _manifest = runtime.session_manifest.lock().expect("manifest lock");
        panic!("poison manifest lock");
    }));

    let error = runtime
        .set_current_mode_id("acceptEdits")
        .expect_err("a poisoned manifest cannot accept a mode update");
    assert_eq!(
        error.message,
        "Session manifest is unavailable; mode change was not applied."
    );
}

#[test]
fn mode_update_rejects_a_missing_manifest() {
    let runtime = SessionRuntime::new();

    let error = runtime
        .set_current_mode_id("acceptEdits")
        .expect_err("a missing manifest cannot accept a mode update");
    assert_eq!(
        error.message,
        "Session mode manifest is missing; mode change was not applied."
    );
}

#[test]
fn mode_update_reports_a_closed_stream() {
    let runtime = SessionRuntime::new();
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: Some("claude".to_string()),
        current_model_id: None,
        models: Vec::new(),
        modes: Some(devboule_protocol::SessionModeStateView {
            current_mode_id: "default".to_string(),
            available_modes: Vec::new(),
        }),
    });
    runtime
        .set_current_mode_id("bypassPermissions")
        .expect("mode");
    runtime.set_current_mode_id("plan").expect("plan mode");
    let before = runtime.session_manifest();
    let previous = runtime.mode_before_plan_id();
    runtime.stream.lock().expect("stream lock").output_closed = true;

    let error = runtime
        .set_current_mode_id("acceptEdits")
        .expect_err("a closed stream cannot deliver the mode update");
    assert_eq!(
        error.message,
        "Session event stream is unavailable; mode change was not applied."
    );
    assert_eq!(runtime.session_manifest(), before);
    assert_eq!(runtime.mode_before_plan_id(), previous);
}

#[test]
fn turn_activity_uses_a_generation_token_and_clears_on_finish() {
    let runtime = SessionRuntime::new();
    assert!(!runtime.is_turn_active(0));
    runtime.begin_turn();
    let turn = runtime.turn_counter();
    assert!(runtime.is_turn_active(turn));
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert!(!runtime.is_turn_active(turn));
    assert_ne!(runtime.turn_counter(), turn);
}

#[test]
fn session_notice_is_emitted_without_changing_runtime_status() {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.stream.lock().unwrap().screen = None;
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.notice.emit",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    assert!(matches!(
        runtime.stream.lock().unwrap().disposition,
        Disposition::Running
    ));

    assert!(runtime.publish_session_notice(
        "Codex declined an out-of-scope request.".to_string(),
        NoticeSeverity::Info,
    ));

    assert!(matches!(
        runtime.stream.lock().unwrap().disposition,
        Disposition::Running
    ));
    assert!(matches!(
        pull_events(&conn).as_slice(),
        [SessionEvent::SessionNotice { text, severity }]
            if text == "Codex declined an out-of-scope request."
                && *severity == NoticeSeverity::Info
    ));
}

#[test]
fn session_notice_survives_detach_and_reattach() {
    let dir = crate::test_dirs::test_temp_dir("devboule-session-notice");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    journal
        .upsert_blocking(SessionRecord {
            id: "s.notice.reattach".to_string(),
            owner: "owner".to_string(),
            workspace_id: None,
            cwd: None,
            kind: SessionKind::Acp,
            provider: None,
            title: "Notice".to_string(),
            created_at_ms: 1,
            updated_at_ms: 1,
            generation: 1,
            status: PersistStatus::Live,
            exit_code: None,
            closed: false,
            last_seq: 0,
            degraded: false,
            dropped_frames: 0,
            dropped_bytes: 0,
            payload_bytes: 0,
            trimmed_bytes: 0,
            reaped: false,
            peer_session_id: None,
            disowned_peer_session_id: None,
            origin: devboule_protocol::SessionOrigin::local(),
            // The row this test reattaches carries no name and no parent:
            // neither is what the notice path is about.
            display_name: None,
            created_by: None,
            profile_id: None,
            context_id: None,
            // The marker this fixture is silent about: the notice path
            // reads nothing from it, and `unknown` says exactly that.
            unattended_state: devboule_protocol::UnattendedState::Unknown,
            labels: Default::default(),
            // No overlay: an end-marker upsert carries NULL, so the
            // birth value stays (the upsert keeps it on NULL).
            overlay: None,
            // No depth either, same rule: the birth value stays.
            depth: None,
            // No goal either, same rule: the goal road writes the column,
            // never a lifecycle upsert.
            goal: None,
        })
        .expect("session row");
    let runtime = Arc::new(SessionRuntime::with_journal(
        "s.notice.reattach".to_string(),
        Some(Arc::clone(&journal)),
    ));
    runtime.stream.lock().unwrap().screen = None;

    let first = ConnHandle::new(1);
    let first_outcome = runtime
        .try_attach_with_replay(None, &first, false)
        .expect("first attach");
    first.track_with_agent_replay(
        "s.notice.reattach",
        Arc::clone(&runtime),
        false,
        None,
        first_outcome.generation,
        first_outcome.live_agent_replay,
    );
    let _ = pull_events(&first);
    runtime.publish_session_notice("mode note".to_string(), NoticeSeverity::Warning);
    assert!(pull_events(&first).iter().any(|event| matches!(
        event,
        SessionEvent::SessionNotice { text, severity }
            if text == "mode note" && *severity == NoticeSeverity::Warning
    )));
    journal.flush().expect("notice flush");
    runtime.detach_subscription(first.id, first.id);
    first.untrack_subscription(first.id);

    let second = ConnHandle::new(2);
    let second_outcome = runtime
        .try_attach_with_replay(None, &second, false)
        .expect("reattach");
    second.track_with_agent_replay(
        "s.notice.reattach",
        Arc::clone(&runtime),
        false,
        None,
        second_outcome.generation,
        second_outcome.live_agent_replay,
    );
    let replayed = pull_events(&second);
    assert_eq!(
        replayed
            .iter()
            .filter(|event| matches!(event, SessionEvent::SessionNotice { .. }))
            .count(),
        1
    );
    assert!(replayed.iter().any(|event| matches!(
        event,
        SessionEvent::SessionNotice { text, severity }
            if text == "mode note" && *severity == NoticeSeverity::Warning
    )));
    let recovered = SessionRuntime::from_replay(
        "s.notice.reattach".to_string(),
        Some(Arc::clone(&journal)),
        journal.replay("s.notice.reattach").expect("replay"),
    );
    let third = ConnHandle::new(3);
    let third_outcome = recovered
        .try_attach_with_replay(None, &third, false)
        .expect("recovered attach");
    third.track_with_agent_replay(
        "s.notice.reattach",
        Arc::clone(&recovered),
        true,
        None,
        third_outcome.generation,
        third_outcome.live_agent_replay,
    );
    assert!(pull_events(&third).iter().any(|event| matches!(
        event,
        SessionEvent::SessionNotice { text, severity }
            if text == "mode note" && *severity == NoticeSeverity::Warning
    )));
    journal.shutdown();
}

/// A transcript rebuilt **with** history has no provenance to carry: the
/// runtime starts fail-closed on the daemon's own `restored` hop — from the
/// journal's last seq or from the events it holds, either signal is history —
/// and a replay that holds nothing starts clean.
#[test]
fn a_replay_with_history_starts_restored_and_an_empty_one_stays_clean() {
    let replay = |last_seq: u64, events: Vec<SessionEvent>| crate::journal::Replay {
        generation: 1,
        last_seq,
        integrity: TranscriptIntegrity::Complete,
        event_seqs: Vec::new(),
        event_ts_ms: Vec::new(),
        events,
    };
    let notice = SessionEvent::SessionNotice {
        text: "from the old life".to_string(),
        severity: NoticeSeverity::Info,
    };
    for (label, built) in [
        ("a last seq", replay(3, Vec::new())),
        ("an event", replay(0, vec![notice])),
    ] {
        let chain = SessionRuntime::from_replay("s.restore.history".to_string(), None, built)
            .ingress_chain();
        assert!(
            chain.is_tainted(),
            "{label} is history: the restore fails closed"
        );
        assert_eq!(
            chain.hops(),
            vec!["restored".to_string()],
            "{label} names the restore, nothing more"
        );
    }
    let never_spoken =
        SessionRuntime::from_replay("s.restore.empty".to_string(), None, replay(0, Vec::new()))
            .ingress_chain();
    assert!(
        !never_spoken.is_tainted() && never_spoken.hops().is_empty(),
        "a replay that holds nothing is no history"
    );
}

#[test]
fn plan_rows_and_agent_report_outcomes_replay_from_the_journal() {
    let dir = crate::test_dirs::test_temp_dir("devboule-plan-decision-replay");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    let session_id = "s.plan.replay";
    let mut record =
        crate::journal::new_session_record(session_id, "owner", None, SessionKind::Claude, "Plan");
    record.closed = false;
    journal.upsert_blocking(record).expect("session row");
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));

    let large_plan = format!("ab{}", "🧭".repeat(crate::plan_text::MAX_PLAN_BYTES));
    let plans = [
        ("approved-plan", "Review the changes"),
        ("rejected-plan", "Run the migration"),
        ("withdrawn-plan", "Remove the old API"),
        ("unanswered-plan", "Wait for a decision"),
        ("oversized-plan", large_plan.as_str()),
    ];
    let envelope = serde_json::json!({
        "type": "assistant",
        "message": {
            "id": "message-plan-replay",
            "role": "assistant",
            "content": plans.iter().map(|(id, plan)| serde_json::json!({
                "type": "tool_use",
                "id": id,
                "name": "ExitPlanMode",
                "input": {"plan": plan},
            })).collect::<Vec<_>>(),
        },
    });
    runtime
        .journal_agent_envelope(&envelope)
        .expect("journal Claude row");

    for (tool_call_id, status, title) in [
        ("approved-plan", "completed", "Approved"),
        ("rejected-plan", "failed", "Rejected"),
        ("withdrawn-plan", "cancelled", "Withdrawn"),
    ] {
        assert!(runtime.publish_daemon_event(SessionEvent::AgentToolUpdate {
            tool_call_id: tool_call_id.to_string(),
            status: Some(status.to_string()),
            text: None,
            title: Some(title.to_string()),
            kind: Some("plan".to_string()),
            locations: None,
            parent_tool_use_id: None,
            spawn_depth: None,
            command: None,
            exit_code: None,
            replace: false,

            images: Vec::new(),
        }));
    }
    journal.flush().expect("flush decision rows");

    let recovered = SessionRuntime::from_replay(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
        journal.replay(session_id).expect("replay"),
    );
    let connection = ConnHandle::new(1);
    let attached = recovered
        .try_attach_with_replay(None, &connection, false)
        .expect("recovered attach");
    connection.track_with_agent_replay(
        session_id,
        Arc::clone(&recovered),
        true,
        None,
        attached.generation,
        attached.live_agent_replay,
    );
    let replayed = pull_events(&connection);
    for (tool_call_id, plan) in plans {
        let expected_plan = crate::plan_text::bound_plan_text(plan);
        assert!(replayed.iter().any(|event| matches!(event,
            SessionEvent::AgentToolUpdate { tool_call_id: id, text: Some(text), kind: Some(kind), .. }
            if id == tool_call_id && text == &expected_plan && kind == "plan"
        )), "missing re-derived plan body for {tool_call_id}");
    }
    for (tool_call_id, status, title) in [
        ("approved-plan", "completed", "Approved"),
        ("rejected-plan", "failed", "Rejected"),
        ("withdrawn-plan", "cancelled", "Withdrawn"),
    ] {
        assert!(replayed.iter().any(|event| matches!(event,
            SessionEvent::AgentToolUpdate { tool_call_id: id, status: Some(actual_status), text: None, title: Some(actual_title), kind: Some(kind), .. }
            if id == tool_call_id && actual_status == status && actual_title == title && kind == "plan"
        )), "missing AgentReport outcome for {tool_call_id}");
    }
    assert!(
        !replayed.iter().any(|event| matches!(event,
            SessionEvent::AgentToolUpdate { tool_call_id, title: Some(title), .. }
            if tool_call_id == "unanswered-plan" && title == "Withdrawn"
        )),
        "replay must leave the unanswered card unanswered"
    );
    assert!(
        replayed.iter().any(|event| matches!(event,
            SessionEvent::AgentToolCall { tool_call_id, status, kind: Some(kind), .. }
            if tool_call_id == "unanswered-plan" && status == "pending" && kind == "plan"
        )),
        "the unanswered plan remains pending for the ended-transcript renderer"
    );
    journal.shutdown();
}

/// The first prompt is owed exactly once per session, and a session built
/// from a replay is owed none: the standing instructions are a session-start
/// rule, never a resume rule.
#[test]
fn a_session_owes_its_first_prompt_once_and_a_replay_owes_none() {
    let runtime = SessionRuntime::new();
    assert!(
        runtime.take_first_prompt(),
        "a session being started owes its first prompt"
    );
    assert!(
        !runtime.take_first_prompt(),
        "and the flag is taken, not read: a second prompt cannot carry a second copy"
    );

    let dir = crate::test_dirs::test_temp_dir("devboule first prompt");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    let mut record = crate::journal::new_session_record(
        "s.replayed",
        "owner",
        None,
        SessionKind::Acp,
        "Replayed",
    );
    record.status = crate::journal::PersistStatus::Ended;
    record.closed = false;
    journal.upsert_blocking(record).expect("the row");
    let recovered = SessionRuntime::from_replay(
        "s.replayed".to_string(),
        Some(Arc::clone(&journal)),
        journal.replay("s.replayed").expect("replay"),
    );
    assert!(
        !recovered.take_first_prompt(),
        "a session that comes back with a transcript already had its first prompt"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// Teardown's failed-termination fallback rests on this one fact: the
/// OS-death closure owns an Arc of the job, and releasing the slot drops
/// the closure with everything it held — so teardown's own drop *can* be
/// the last handle. Any holder the release cannot reach keeps the job
/// until it finishes, and every holder runs its own `terminate()`. The
/// test drives the after-failed-wait step production calls, so deleting
/// the release inside it fails here.
#[test]
fn releasing_the_os_death_cascade_drops_what_its_closure_held() {
    let runtime = SessionRuntime::new();
    let held = Arc::new(());
    runtime.set_on_os_death(Arc::new({
        let held = Arc::clone(&held);
        move || drop(Arc::clone(&held))
    }));
    assert_eq!(Arc::strong_count(&held), 2, "the cascade holds one Arc");
    super::super::session_spawn::release_after_failed_wait(&runtime);
    assert_eq!(
        Arc::strong_count(&held),
        1,
        "the released cascade is gone with everything it held"
    );
}

/// An agent error raises error attention: the premise the failed-turn
/// test relies on, pinned without threads or timing.
#[test]
fn agent_error_raises_error_attention() {
    let runtime = SessionRuntime::new();
    runtime.publish_agent_event(
        SessionEvent::AgentError {
            message: "ACP request 1 failed: stub turn failure".to_string(),
        },
        None,
    );
    assert!(
        matches!(runtime.attention(), Some(raised) if matches!(raised.reason, AttentionReason::Error)),
        "an agent error raises error attention"
    );
}

/// A finished turn never overwrites a standing error raise: first raise
/// wins, the way the roster's priority order reads. This is what makes
/// the failed turn's final snapshot deterministic.
#[test]
fn agent_finished_does_not_overwrite_error_attention() {
    let runtime = SessionRuntime::new();
    runtime.publish_agent_event(
        SessionEvent::AgentError {
            message: "ACP request 1 failed: stub turn failure".to_string(),
        },
        None,
    );
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "error".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert!(
        matches!(runtime.attention(), Some(raised) if matches!(raised.reason, AttentionReason::Error)),
        "the turn's finish leaves the error raise standing"
    );
}

#[test]
fn failed_plan_mark_read_notices_once_instead_of_empty() {
    // A journal that cannot answer: the marks come back empty, but not
    // silently — exactly one SessionNotice names the gap in the backlog.
    let dir = crate::test_dirs::test_temp_dir("devboule-plan-mark-fail");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    let session_id = "s.plan.mark.fail";
    journal.shutdown();
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    assert!(matches!(
        runtime.journal_lookback(LookbackRequest::CodexPlanMarks),
        LookbackAnswer::PlanMarks(marks) if marks.is_empty()
    ));
    let notices = runtime
        .stream
        .lock()
        .expect("stream")
        .agent_backlog
        .iter()
        .filter(|item| {
            matches!(item, crate::session::PendingItem::Agent { event, .. }
                if matches!(event, SessionEvent::SessionNotice { text, .. } if text.contains("plan approval")))
        })
        .count();
    assert_eq!(notices, 1, "exactly one plan-mark notice");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plan_mark_scans_are_counted() {
    let dir = crate::test_dirs::test_temp_dir("devboule-plan-mark-count");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    let session_id = "s.plan.mark.count";
    journal.shutdown();
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    assert!(matches!(
        runtime.journal_lookback(LookbackRequest::CodexPlanMarks),
        LookbackAnswer::PlanMarks(marks) if marks.is_empty()
    ));
    assert_eq!(runtime.plan_mark_scan_count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_turn_begun_between_a_settled_finish_and_its_publish_survives() {
    let runtime = SessionRuntime::new();
    runtime.begin_turn();
    // The result path settles the turn before publishing it; the window this
    // opens is exactly one begin_turn wide.
    runtime.settle_turn_finish(|| false);
    runtime.begin_turn();
    runtime.publish_agent_event_settled_with_seq(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert!(
        runtime.is_running_turn(),
        "the publish of an already-settled finish must not end the turn that began after it"
    );
}

#[test]
fn an_unsetled_publish_still_ends_the_turn() {
    // Every provider but the settled result path relies on this: publishing
    // an AgentFinished through the generic route is what ends its turn.
    let runtime = SessionRuntime::new();
    runtime.begin_turn();
    runtime.publish_agent_event_with_seq(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
        None,
    );
    assert!(!runtime.is_running_turn());
}

/// The slice's headline invariant: the echo's `at_ms` and its row's `ts_ms`
/// are one reading of the journal's clock, and replay carries that same
/// instant back — so a turn cannot change its time across a restart.
#[test]
fn the_user_echo_and_its_journal_row_name_one_time() {
    let dir = crate::test_dirs::test_temp_dir("devboule-echo-one-time");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    let session_id = "s.echo.one.time";
    let mut record = crate::journal::new_session_record(
        session_id,
        "S-1-5-21-1",
        None,
        SessionKind::Acp,
        "Agent",
    );
    record.closed = false;
    journal.upsert_blocking(record).expect("session row");
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.generation = 1;
        stream.next_seq = 1;
    }
    runtime
        .publish_agent_user_message(
            "one clock".to_string(),
            devboule_protocol::UserMessageAuthor::Human,
            devboule_protocol::UserMessageKind::Composer,
        )
        .expect("published");
    journal.flush().expect("flush the row");

    let row = journal
        .replay_agent_page(session_id, 1, 0, 0, u64::MAX, 100)
        .expect("rows")
        .records
        .into_iter()
        .find(|record| {
            matches!(
                serde_json::from_slice::<devboule_protocol::SessionEvent>(&record.payload),
                Ok(devboule_protocol::SessionEvent::AgentUserMessage { .. })
            )
        })
        .expect("the echo row");
    let live_at_ms = match serde_json::from_slice::<devboule_protocol::SessionEvent>(&row.payload) {
        Ok(devboule_protocol::SessionEvent::AgentUserMessage { at_ms, .. }) => at_ms,
        other => panic!("expected the echo row, got {other:?}"),
    };
    assert_eq!(
        live_at_ms,
        Some(row.ts_ms),
        "one reading of the clock for the echo and its row"
    );

    let recovered = SessionRuntime::from_replay(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
        journal.replay(session_id).expect("replay"),
    );
    let connection = ConnHandle::new(1);
    let attached = recovered
        .try_attach_with_replay(None, &connection, false)
        .expect("recovered attach");
    connection.track_with_agent_replay(
        session_id,
        Arc::clone(&recovered),
        true,
        None,
        attached.generation,
        attached.live_agent_replay,
    );
    let replayed_at = pull_events(&connection)
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentUserMessage { at_ms, .. } => Some(*at_ms),
            _ => None,
        })
        .expect("the echo is replayed");
    assert_eq!(
        replayed_at,
        Some(row.ts_ms),
        "replay carries the row's instant, not a second stamping"
    );

    drop(runtime);
    drop(journal);
    drop(recovered);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A kind that is not a user turn carries no turn time on either path: the
/// echo does not stamp it live, and replay does not fill it — the two rules
/// agree, so a relay never grows a time across a restart.
#[test]
fn a_non_turn_kind_carries_no_time_live_or_on_replay() {
    let dir = crate::test_dirs::test_temp_dir("devboule-non-turn-time");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    let session_id = "s.non.turn.time";
    let mut record = crate::journal::new_session_record(
        session_id,
        "S-1-5-21-1",
        None,
        SessionKind::Acp,
        "Agent",
    );
    record.closed = false;
    journal.upsert_blocking(record).expect("session row");
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.generation = 1;
        stream.next_seq = 1;
    }
    runtime
        .publish_agent_user_message(
            "relayed words".to_string(),
            devboule_protocol::UserMessageAuthor::Agent,
            devboule_protocol::UserMessageKind::OutgoingA2a,
        )
        .expect("published");
    journal.flush().expect("flush the row");

    let row = journal
        .replay_agent_page(session_id, 1, 0, 0, u64::MAX, 100)
        .expect("rows")
        .records
        .into_iter()
        .find(|record| {
            matches!(
                serde_json::from_slice::<devboule_protocol::SessionEvent>(&record.payload),
                Ok(devboule_protocol::SessionEvent::AgentUserMessage { .. })
            )
        })
        .expect("the relay row");
    let live_at_ms = match serde_json::from_slice::<devboule_protocol::SessionEvent>(&row.payload) {
        Ok(devboule_protocol::SessionEvent::AgentUserMessage { at_ms, .. }) => at_ms,
        other => panic!("expected the relay row, got {other:?}"),
    };
    assert_eq!(
        live_at_ms, None,
        "a non-turn kind is not stamped live, though the clock was read"
    );

    let recovered = SessionRuntime::from_replay(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
        journal.replay(session_id).expect("replay"),
    );
    let connection = ConnHandle::new(1);
    let attached = recovered
        .try_attach_with_replay(None, &connection, false)
        .expect("recovered attach");
    connection.track_with_agent_replay(
        session_id,
        Arc::clone(&recovered),
        true,
        None,
        attached.generation,
        attached.live_agent_replay,
    );
    let replayed_at = pull_events(&connection)
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentUserMessage { at_ms, .. } => Some(*at_ms),
            _ => None,
        })
        .expect("the relay is replayed");
    assert_eq!(
        replayed_at, None,
        "replay does not fill a non-turn kind either"
    );

    drop(runtime);
    drop(journal);
    drop(recovered);
    let _ = std::fs::remove_dir_all(&dir);
}
