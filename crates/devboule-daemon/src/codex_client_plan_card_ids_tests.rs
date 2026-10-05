//! The card's id: the card, its tool row and its journal id are one id — the
//! plan item's own `<turnId>-plan` — so the rows merge live and on replay.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use devboule_protocol::{PermissionOutcome, PermissionRequestKind, SessionEvent};

use super::plan_card_test_support::{frames_for_turn, plan_reader, published, SESSION, THREAD};

#[test]
fn two_spawns_of_one_session_each_register_their_plan_card() {
    // Generation 1: turn T1's card is raised and answered, so the journal
    // closes it. The resume reuses the same session row; generation 2's turn
    // T2 must mint a fresh id, not re-mint T1's.
    let dir = crate::test_dirs::test_temp_dir("devboule-codex-plan-card-resume");
    let path = dir.join("broker.sqlite");
    let journal = Arc::new(crate::journal::Journal::open(&path).expect("journal"));
    journal
        .upsert_blocking(crate::journal::new_session_record(
            SESSION.to_string(),
            "owner",
            None,
            devboule_protocol::SessionKind::Codex,
            "codex plan card resume test",
        ))
        .expect("session row");
    let runtime = Arc::new(
        crate::session::session_runtime::SessionRuntime::with_journal(
            SESSION.to_string(),
            Some(journal),
        ),
    );
    runtime.stream.lock().unwrap().screen = None;
    let conn = crate::session::event_pull::ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        SESSION,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let broker =
        crate::session::permission_broker::PermissionBroker::with_sender(Arc::new(|_, _| Ok(())));

    let mut seen_cards = Vec::new();
    for (generation, turn_id) in (1..).zip(["turn-generation-1", "turn-generation-2"]) {
        let state = Arc::new(crate::codex_view::CodexState::new(
            THREAD.to_string(),
            crate::codex_view::catalog_from_response(&serde_json::json!({
                "data": [{ "id": "model", "isDefault": true }]
            }))
            .expect("catalog"),
            "auto",
        ));
        state
            .set_collaboration_modes(&serde_json::json!({
                "data": [
                    { "name": "Plan", "mode": "plan" },
                    { "name": "Default", "mode": "default" }
                ]
            }))
            .expect("modes");
        let commands = super::empty_commands();
        let stdin = Arc::new(Mutex::new(None));
        let next_id = Arc::new(AtomicU64::new(1));
        let plan_prompt = Arc::new(super::CodexStaticPrompt::new(
            Arc::clone(&stdin),
            Arc::clone(&next_id),
            Arc::clone(&state),
            Arc::clone(&commands),
        ));
        let mut reader = super::CodexReader {
            images: None,
            commands,
            available_commands: None,
            buffer: Vec::new(),
            discarding_oversized_line: false,
            deferred: Vec::new(),
            manifest: None,
            state,
            view: crate::codex_view::CodexView::new(None),
            permission_broker: Arc::clone(&broker),
            response_ids: Arc::new(Mutex::new(HashMap::new())),
            stdin,
            next_id,
            requests: Arc::new(super::CodexRequests::new()),
            compactions: crate::codex_compaction::CodexCompactions::default(),
            plan_prompt,
        };
        // This generation's turn: a plan turn under its own id.
        let frames = frames_for_turn(turn_id);
        reader.state.record_turn_start("d-1", true);
        reader.dispatch_value(
            serde_json::json!({
                "method": "turn/started",
                "params": {"threadId": THREAD, "turn": {"id": turn_id}}
            }),
            &runtime,
        );
        for frame in &frames {
            reader.dispatch_value(frame.clone(), &runtime);
        }
        // The card's id is discovered from the registration itself, so a
        // generation that fails to register one is caught here.
        let card_id = published(&conn)
            .iter()
            .find_map(|event| match event {
                SessionEvent::PermissionRequest {
                    tool_call_id,
                    kind: Some(PermissionRequestKind::Plan),
                    ..
                } => Some(tool_call_id.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("generation {generation} registers its own card"));
        assert_eq!(
            card_id,
            format!("{turn_id}-plan"),
            "the card is keyed on the plan item's own id"
        );
        seen_cards.push(card_id.clone());
        // Answer it, so the journal closes this generation's id.
        broker
            .respond_with_option(
                &card_id,
                PermissionOutcome::Deny,
                Some("deny".to_string()),
                None,
            )
            .expect("the card answers");
        let _ = conn.pull_events();
    }
    assert_eq!(
        seen_cards,
        vec![
            "turn-generation-1-plan".to_string(),
            "turn-generation-2-plan".to_string(),
        ],
        "each generation's card is keyed on its own turn id"
    );
}

#[test]
fn a_completion_without_a_turn_id_folds_the_row_and_raises_no_card() {
    let (mut reader, runtime, conn, broker) = plan_reader();
    let frames = crate::codex_view::fixture_frames(include_str!(
        "../fixtures/wire/codex/codex-plan-items-live.jsonl"
    ));
    let thread_id = frames[2]["params"]["threadId"]
        .as_str()
        .expect("thread id")
        .to_string();
    reader.state.record_turn_start("d-1", true);
    reader.dispatch_value(
        serde_json::json!({
            "method": "turn/started",
            "params": {"threadId": thread_id, "turn": {"id": "turn-no-id"}}
        }),
        &runtime,
    );
    // The plan item frames, then a completion that carries a status but no
    // turn id.
    reader.dispatch_value(frames[0].clone(), &runtime);
    reader.dispatch_value(frames[1].clone(), &runtime);
    reader.dispatch_value(
        serde_json::json!({
            "method": "turn/completed",
            "params": {"threadId": thread_id, "turn": {"status": "interrupted"}}
        }),
        &runtime,
    );

    let events = published(&conn);
    assert_eq!(
        broker.pending_len(),
        0,
        "no card is raised without a turn id"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::PermissionRequest { .. })),
        "no card is published: {events:?}"
    );
    assert!(
        events.iter().any(
            |event| matches!(event, SessionEvent::AgentToolUpdate { kind: Some(kind), text: Some(text), .. }
            if kind == "plan" && text.contains("hello.txt"))
        ),
        "the plan text folds into the row: {events:?}"
    );
}
