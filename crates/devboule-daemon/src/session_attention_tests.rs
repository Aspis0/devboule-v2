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
