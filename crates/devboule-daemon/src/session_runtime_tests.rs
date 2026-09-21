//! Tests for the session runtime: turn tokens, attention state and the agent snapshot.

use super::super::event_pull::ConnHandle;
use super::*;
use crate::journal::{PersistStatus, SessionRecord};
use devboule_protocol::{NoticeSeverity, SessionModeStateView, SessionModeView};

fn pull_events(conn: &ConnHandle) -> Vec<SessionEvent> {
    conn.pull_events()
        .into_iter()
        .map(|pending| pending.envelope.event)
        .collect()
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
fn turn_activity_uses_a_generation_token_and_clears_on_finish() {
    let runtime = SessionRuntime::new();
    assert!(!runtime.is_turn_active(0));
    runtime.begin_turn();
    let turn = runtime.turn_counter();
    assert!(runtime.is_turn_active(turn));
    runtime.finish_turn();
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
    let dir = std::env::temp_dir().join(format!(
        "devboule-session-notice-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
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

    let dir = std::env::temp_dir().join(format!(
        "devboule first prompt {}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
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
