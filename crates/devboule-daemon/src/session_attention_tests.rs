//! Attention and suppression, moved whole out of `session_tests.rs` lines
//! 2527-2797: priority preserving permission while allowing escalation, a
//! clear that cannot complete inside the suppression decision, focus that
//! suppresses attention while presence clears it, presence that raises unless
//! the second connection is looking somewhere else, and a prompt or an answer
//! that acknowledges attention. Every line below is byte-identical to its text
//! there apart from this header; `permission_attention_event` and
//! `insert_live_agent_with_writer` are `pub(super)` in the provider this file
//! imports them from.

use super::tests::{
    insert_live_agent, insert_live_agent_with_writer, permission_attention_event, test_owner,
    tmp_delete_registry, RecordingWriter,
};
use super::*;

#[test]
fn attention_priority_preserves_permission_and_allows_escalation() {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    let finished_at = runtime.attention().expect("finished attention");
    assert_eq!(
        finished_at.reason,
        devboule_protocol::AttentionReason::Finished
    );
    std::thread::sleep(Duration::from_millis(2));
    runtime.publish_agent_event(
        SessionEvent::AgentError {
            message: "attention error".to_string(),
        },
        None,
    );
    let error_at = runtime.attention().expect("error attention");
    assert_eq!(error_at.reason, devboule_protocol::AttentionReason::Error);
    assert!(error_at.at_ms > finished_at.at_ms);
    runtime.publish_agent_event(permission_attention_event(), None);
    assert_eq!(
        runtime.attention().expect("permission attention").reason,
        devboule_protocol::AttentionReason::Permission
    );
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert_eq!(
        runtime
            .attention()
            .expect("permission stays pending")
            .reason,
        devboule_protocol::AttentionReason::Permission
    );
}

#[test]
fn attention_clear_cannot_complete_during_the_suppression_decision() {
    let runtime = Arc::new(SessionRuntime::new());
    let suppression_entered = Arc::new(std::sync::Barrier::new(2));
    let release_suppression = Arc::new(std::sync::Barrier::new(2));
    let entered = Arc::clone(&suppression_entered);
    let release = Arc::clone(&release_suppression);
    runtime.set_attention_hooks(
        Arc::new(move || {
            entered.wait();
            release.wait();
            false
        }),
        Arc::new(|| Box::new(|| {}) as Box<dyn FnOnce() + Send>),
    );

    let raising = Arc::clone(&runtime);
    let raise_thread = std::thread::spawn(move || {
        raising.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
    });
    suppression_entered.wait();

    let (clear_started, clear_started_rx) = std::sync::mpsc::channel();
    let (clear_done, clear_done_rx) = std::sync::mpsc::channel();
    let clearing = Arc::clone(&runtime);
    let clear_thread = std::thread::spawn(move || {
        clear_started.send(()).expect("clear thread started");
        clear_done
            .send(clearing.clear_attention())
            .expect("clear result");
    });
    clear_started_rx
        .recv()
        .expect("clear thread reached the call");
    let clear_was_blocked = clear_done_rx
        .recv_timeout(Duration::from_millis(100))
        .is_err();

    release_suppression.wait();
    raise_thread.join().expect("raise thread");
    clear_thread.join().expect("clear thread");
    assert!(
        clear_was_blocked,
        "clear completed while the suppression decision was still open"
    );
    assert!(runtime.attention().is_none());
}

#[test]
fn visible_focus_suppresses_attention_and_presence_clears_it() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attention", "process-attention");
    let runtime = insert_live_agent(&registry, "s.attention.1", owner.clone());
    registry
        .set_presence(1, &owner, Some("s.attention.1".to_string()), true)
        .expect("presence");
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert!(
        runtime.attention().is_none(),
        "visible focus suppresses raise"
    );
    registry.clear_presence(1);
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert!(runtime.attention().is_some());
    registry
        .set_presence(1, &owner, Some("s.attention.1".to_string()), true)
        .expect("focus clears attention");
    assert!(
        runtime.attention().is_none(),
        "focus acknowledges attention"
    );
    drop(journal);
    let _ = std::fs::remove_dir_all(_dir);
}

#[test]
fn invisible_presence_raises_and_a_second_connection_elsewhere_does_not_suppress() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-presence", "process-presence");
    let runtime = insert_live_agent(&registry, "s.presence.1", owner.clone());
    registry
        .set_presence(1, &owner, None, false)
        .expect("invisible presence");
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert!(
        runtime.attention().is_some(),
        "invisible app is not watching"
    );
    assert!(runtime.clear_attention());
    registry
        .set_presence(1, &owner, Some("s.presence.1".to_string()), true)
        .expect("focused connection");
    registry
        .set_presence(2, &owner, Some("s.other.1".to_string()), true)
        .expect("second connection elsewhere");
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert!(
        runtime.attention().is_none(),
        "the focused connection suppresses"
    );
    drop(journal);
    let _ = std::fs::remove_dir_all(_dir);
}

#[test]
fn sending_a_prompt_acknowledges_attention() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-send-attention", "process-send-attention");
    let runtime = insert_live_agent_with_writer(
        &registry,
        "s.send.1",
        owner.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    let conn = ConnHandle::new(7);
    registry
        .attach("s.send.1", None, &conn, &owner, true)
        .expect("attach");
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert!(runtime.attention().is_some());
    registry
        .send("s.send.1", "next", &owner, &conn)
        .expect("send");
    assert!(
        runtime.attention().is_none(),
        "prompt acknowledges attention"
    );
    drop(journal);
    let _ = std::fs::remove_dir_all(_dir);
}

#[test]
fn answering_permission_acknowledges_attention() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner(
        "S-1-5-21-permission-attention",
        "process-permission-attention",
    );
    let session_id = "s.permission-attention.1";
    let runtime = insert_live_agent(&registry, session_id, owner.clone());
    journal
        .upsert_blocking(new_session_record(
            session_id,
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("session row");
    let conn = ConnHandle::new(8);
    registry
        .attach(session_id, None, &conn, &owner, true)
        .expect("attach");
    let request = permission_broker::permission("ack-permission");
    runtime.publish_agent_event(request.clone(), None);
    runtime
        .permission_broker()
        .expect("permission broker")
        .register(12, request, &runtime)
        .expect("permission request");
    assert_eq!(
        runtime.attention().expect("permission attention").reason,
        devboule_protocol::AttentionReason::Permission
    );

    registry
        .permission_respond(
            session_id,
            "ack-permission",
            PermissionOutcome::AllowOnce,
            &conn,
            &owner,
        )
        .expect("permission response");
    assert!(
        runtime.attention().is_none(),
        "answering permission acknowledges attention"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// Pushed roster rows, one entry per transition the sink saw. Installed
/// after the setup so only the withdrawal's own pushes are counted.
fn install_sink(registry: &SessionRegistry) -> Arc<Mutex<Vec<Vec<SessionStateSnapshot>>>> {
    let log: Arc<Mutex<Vec<Vec<SessionStateSnapshot>>>> = Arc::new(Mutex::new(Vec::new()));
    let fired = Arc::clone(&log);
    registry.set_transition_sink(Arc::new(move |_owner, snapshots| {
        fired
            .lock()
            .expect("sink log")
            .push(snapshots.unwrap_or_default());
    }));
    log
}

/// Every pushed row for this session: a stale first push fails here even
/// when the last row is already clean.
fn pushed_rows_for(
    sink: &Arc<Mutex<Vec<Vec<SessionStateSnapshot>>>>,
    session_id: &str,
) -> Vec<SessionStateSnapshot> {
    sink.lock()
        .expect("sink log")
        .iter()
        .flatten()
        .filter(|snapshot| snapshot.id == session_id)
        .cloned()
        .collect()
}
/// The last pushed row for this session. A push that never carried the row
/// fails here, so a missing row cannot pass as a cleared attention.
fn last_pushed_row(
    sink: &Arc<Mutex<Vec<Vec<SessionStateSnapshot>>>>,
    session_id: &str,
) -> SessionStateSnapshot {
    pushed_rows_for(sink, session_id)
        .pop()
        .expect("a push carried the session row")
}

#[test]
fn cancelling_a_parked_card_clears_permission_attention() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-withdraw-attention", "process-withdraw-attention");
    let session_id = "s.withdraw-attention.1";
    let runtime = insert_live_agent(&registry, session_id, owner.clone());
    journal
        .upsert_blocking(new_session_record(
            session_id,
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("session row");
    let conn = ConnHandle::new(11);
    registry
        .attach(session_id, None, &conn, &owner, true)
        .expect("attach");
    let request = permission_broker::permission("withdraw-attention-card");
    runtime.publish_agent_event(request.clone(), None);
    let broker = runtime.permission_broker().expect("permission broker");
    broker
        .register(21, request, &runtime)
        .expect("permission request");
    assert_eq!(
        runtime.attention().expect("permission attention").reason,
        devboule_protocol::AttentionReason::Permission
    );

    let sink = install_sink(&registry);
    broker.cancel_pending();

    assert!(
        runtime.attention().is_none(),
        "withdrawing the last card clears permission attention"
    );
    let rows = pushed_rows_for(&sink, session_id);
    assert_eq!(
        sink.lock().expect("sink log").len(),
        2,
        "the withdrawal pushes twice: the clear, then the resolved card"
    );
    assert!(
        rows.iter().all(|row| row.attention.is_none()),
        "no pushed row carries the stale attention"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn withdrawing_one_of_two_cards_keeps_permission_attention() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-two-cards", "process-two-cards");
    let session_id = "s.two-cards.1";
    let runtime = insert_live_agent(&registry, session_id, owner.clone());
    journal
        .upsert_blocking(new_session_record(
            session_id,
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("session row");
    let conn = ConnHandle::new(12);
    registry
        .attach(session_id, None, &conn, &owner, true)
        .expect("attach");
    let broker = runtime.permission_broker().expect("permission broker");
    let first = permission_broker::permission("two-cards-first");
    runtime.publish_agent_event(first.clone(), None);
    let pending_first = broker.register(22, first, &runtime).expect("first card");
    let second = permission_broker::permission("two-cards-second");
    runtime.publish_agent_event(second.clone(), None);
    broker.register(23, second, &runtime).expect("second card");
    assert_eq!(
        runtime.attention().expect("permission attention").reason,
        devboule_protocol::AttentionReason::Permission
    );

    let sink = install_sink(&registry);
    // Deliberate asymmetry, and the answer side is the wrong one: a wire
    // answer clears unconditionally even with a card still parked, while a
    // withdrawal keeps the pill while one is. Widening the answer door is a
    // separate decision; this test pins the withdrawal half.
    assert!(broker.cancel("two-cards-first", &pending_first, "cancelled"));
    assert!(
        sink.lock().expect("sink log").is_empty(),
        "a partial withdrawal pushes nothing: the pill is still right"
    );

    assert_eq!(
        runtime
            .attention()
            .expect("attention outlives one withdrawal")
            .reason,
        devboule_protocol::AttentionReason::Permission
    );
    broker.cancel_pending();
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn withdrawing_a_card_keeps_a_non_permission_attention() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-error-stays", "process-error-stays");
    let session_id = "s.error-stays.1";
    let runtime = insert_live_agent(&registry, session_id, owner.clone());
    journal
        .upsert_blocking(new_session_record(
            session_id,
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("session row");
    let conn = ConnHandle::new(13);
    registry
        .attach(session_id, None, &conn, &owner, true)
        .expect("attach");
    let request = permission_broker::permission("error-stays-card");
    runtime.publish_agent_event(request.clone(), None);
    let broker = runtime.permission_broker().expect("permission broker");
    broker
        .register(24, request, &runtime)
        .expect("permission request");
    assert!(runtime.clear_attention());
    runtime.publish_agent_event(
        SessionEvent::AgentError {
            message: "attention error".to_string(),
        },
        None,
    );
    assert_eq!(
        runtime.attention().expect("error attention").reason,
        devboule_protocol::AttentionReason::Error
    );

    broker.cancel_pending();

    assert_eq!(
        runtime.attention().expect("error survives").reason,
        devboule_protocol::AttentionReason::Error
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn mcp_cancel_withdraws_the_card_and_clears_permission_attention() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-mcp-withdraw", "process-mcp-withdraw");
    let session_id = "s.mcp-withdraw.1";
    let runtime = insert_live_agent(&registry, session_id, owner.clone());
    journal
        .upsert_blocking(new_session_record(
            session_id,
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("session row");
    let broker = runtime.permission_broker().expect("permission broker");
    let request_id = serde_json::json!(7);
    let cancel_id = request_id.clone();
    let (token_tx, token_rx) = std::sync::mpsc::channel();
    let host_broker = Arc::clone(&broker);
    let host_runtime = Arc::clone(&runtime);
    let waiter = std::thread::spawn(move || {
        let _scope = crate::mcp_broker::McpCallScope::enter_for_test(session_id, &request_id);
        let token = crate::mcp_broker::current_mcp_call()
            .expect("active call")
            .2;
        token_tx.send(Arc::clone(&token)).expect("token send");
        host_broker.request_host_permission(
            permission_broker::permission("mcp-withdraw-card"),
            &host_runtime,
        )
    });
    let token = token_rx.recv().expect("call token");
    let mut spins = 0;
    while runtime.attention().is_none() && spins < 10_000 {
        std::thread::sleep(Duration::from_millis(1));
        spins += 1;
    }
    assert_eq!(broker.pending_len(), 1, "the host card is parked");
    assert_eq!(
        runtime.attention().expect("permission attention").reason,
        devboule_protocol::AttentionReason::Permission
    );

    let sink = install_sink(&registry);
    assert!(broker.cancel_mcp_call(&runtime.session_id, &cancel_id, &token));

    assert_eq!(
        waiter.join().expect("host waiter"),
        permission_broker::HostDecision::Cancelled
    );
    assert!(
        runtime.attention().is_none(),
        "mcp cancel clears permission attention"
    );
    let rows = pushed_rows_for(&sink, session_id);
    assert_eq!(
        sink.lock().expect("sink log").len(),
        2,
        "the withdrawal pushes twice: the clear, then the resolved card"
    );
    assert!(
        rows.iter().all(|row| row.attention.is_none()),
        "no pushed row carries the stale attention"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn closing_the_broker_clears_permission_attention() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-close-attention", "process-close-attention");
    let session_id = "s.close-attention.1";
    let runtime = insert_live_agent(&registry, session_id, owner.clone());
    journal
        .upsert_blocking(new_session_record(
            session_id,
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("session row");
    let conn = ConnHandle::new(14);
    registry
        .attach(session_id, None, &conn, &owner, true)
        .expect("attach");
    let request = permission_broker::permission("close-attention-card");
    runtime.publish_agent_event(request.clone(), None);
    let broker = runtime.permission_broker().expect("permission broker");
    broker
        .register(25, request, &runtime)
        .expect("permission request");
    assert_eq!(
        runtime.attention().expect("permission attention").reason,
        devboule_protocol::AttentionReason::Permission
    );

    let sink = install_sink(&registry);
    broker.close();

    assert!(
        runtime.attention().is_none(),
        "closing the broker clears permission attention"
    );
    let rows = pushed_rows_for(&sink, session_id);
    assert_eq!(
        sink.lock().expect("sink log").len(),
        2,
        "the withdrawal pushes twice: the clear, then the resolved card"
    );
    assert!(
        rows.iter().all(|row| row.attention.is_none()),
        "no pushed row carries the stale attention"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn withdrawing_from_a_dead_session_still_pushes_the_cleared_row() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-dead-attention", "process-dead-attention");
    let session_id = "s.dead-attention.1";
    let runtime = insert_live_agent(&registry, session_id, owner.clone());
    journal
        .upsert_blocking(new_session_record(
            session_id,
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("session row");
    let conn = ConnHandle::new(15);
    registry
        .attach(session_id, None, &conn, &owner, true)
        .expect("attach");
    let request = permission_broker::permission("dead-attention-card");
    runtime.publish_agent_event(request.clone(), None);
    let broker = runtime.permission_broker().expect("permission broker");
    broker
        .register(26, request, &runtime)
        .expect("permission request");
    assert_eq!(
        runtime.attention().expect("permission attention").reason,
        devboule_protocol::AttentionReason::Permission
    );
    runtime.mark_exited(Some(1));
    runtime.close_output();

    let sink = install_sink(&registry);
    broker.cancel_pending();

    assert!(
        runtime.attention().is_none(),
        "the withdrawal clears even with the output closed"
    );
    assert!(
        last_pushed_row(&sink, session_id).attention.is_none(),
        "the clear pushes its own row: the resolved publish is dropped"
    );
    assert_eq!(
        sink.lock().expect("sink log").len(),
        1,
        "the dropped publish pushes nothing: only the clear's own push"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn auto_answer_grants_without_raising_attention() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-auto-attention", "process-auto-attention");
    let session_id = "s.auto-attention.1";
    let runtime = insert_live_agent(&registry, session_id, owner.clone());
    journal
        .upsert_blocking(new_session_record(
            session_id,
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("session row");
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: Some("test".to_string()),
        current_model_id: None,
        models: Vec::new(),
        modes: Some(devboule_protocol::SessionModeStateView {
            current_mode_id: "auto_accept".to_string(),
            available_modes: Vec::new(),
        }),
    });
    let conn = ConnHandle::new(16);
    registry
        .attach(session_id, None, &conn, &owner, true)
        .expect("attach");
    // Production order: every auto_answer call site grants and returns
    // before the PermissionRequest is ever published, so no raise precedes
    // the grant and there is no pill to go stale. This test pins that the
    // grant path raises nothing; it says nothing about a published card.
    let broker = runtime.permission_broker().expect("permission broker");
    broker
        .register(
            27,
            permission_broker::permission("auto-attention-card"),
            &runtime,
        )
        .expect("permission request");
    assert!(
        broker
            .auto_answer("auto-attention-card", &runtime)
            .expect("mode grant"),
        "the mode grants the card"
    );
    assert_eq!(broker.pending_len(), 0);
    assert!(
        runtime.attention().is_none(),
        "the grant raised no attention"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
