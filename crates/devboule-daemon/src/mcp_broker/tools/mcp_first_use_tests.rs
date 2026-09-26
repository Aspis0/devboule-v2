//! Tests for the first-use write gate: one human card per session and group.

use super::*;
use crate::server::ServerState;
use devboule_protocol::{OwnerId, PermissionOutcome, SessionKind};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn owner() -> OwnerId {
    OwnerId::new("S-1-5-21-first-use", "first-use-client").expect("owner")
}

fn session(state: &Arc<ServerState>, id: &str) {
    crate::session::insert_test_live_agent_with_kind(
        &state.sessions,
        id,
        owner(),
        SessionKind::Acp,
    );
}

fn pending_ids(state: &Arc<ServerState>, id: &str) -> Vec<String> {
    state
        .sessions
        .live_runtime(id, &owner())
        .expect("live session")
        .permission_broker()
        .expect("test broker")
        .test_pending_ids()
}

/// The pending gate card, waited for: the call runs on another thread because
/// the gate blocks until a human answers.
fn wait_for_card(state: &Arc<ServerState>, id: &str) -> String {
    let start = Instant::now();
    loop {
        let mut ids = pending_ids(state, id);
        if let Some(id) = ids.pop() {
            return id;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the gate raised no card"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn answer(
    state: &Arc<ServerState>,
    id: &str,
    card: &str,
    outcome: PermissionOutcome,
    option: &str,
) {
    state
        .sessions
        .live_runtime(id, &owner())
        .expect("live session")
        .permission_broker()
        .expect("test broker")
        .test_answer(card, outcome, option)
        .expect("answer the gate card");
}

fn spawn_gate(state: &Arc<ServerState>, id: &str) -> std::thread::JoinHandle<Result<(), String>> {
    let thread_state = Arc::clone(state);
    let id = id.to_string();
    std::thread::spawn(move || {
        ensure_write_allowed(
            &thread_state,
            &thread_state.mcp,
            &id,
            &owner(),
            WORKSPACES_GROUP,
        )
    })
}

#[test]
fn the_first_call_raises_a_card_and_an_allow_opens_the_group() {
    let state = ServerState::new("first-use-allow".to_string());
    session(&state, "fu-allow");

    let handle = spawn_gate(&state, "fu-allow");
    let card = wait_for_card(&state, "fu-allow");
    assert!(
        card.starts_with("write:workspaces:fu-allow:"),
        "the card names the group and the session: {card}"
    );

    // A second call racing the first is refused as pending, not parked twice.
    assert_eq!(
        ensure_write_allowed(&state, &state.mcp, "fu-allow", &owner(), WORKSPACES_GROUP),
        Err("permission pending; retry".to_string())
    );
    assert_eq!(pending_ids(&state, "fu-allow").len(), 1);

    answer(
        &state,
        "fu-allow",
        &card,
        PermissionOutcome::AllowOnce,
        "allow",
    );
    assert!(handle.join().expect("gate thread").is_ok());
    assert_eq!(
        state.mcp.first_use_mark("fu-allow", WORKSPACES_GROUP),
        Some(GateMark::Open)
    );

    // The second call raises no card: the group stays allowed for the session.
    assert!(
        ensure_write_allowed(&state, &state.mcp, "fu-allow", &owner(), WORKSPACES_GROUP).is_ok()
    );
    assert!(pending_ids(&state, "fu-allow").is_empty());
}

#[test]
fn a_deny_refuses_and_leaves_the_gate_shut() {
    let state = ServerState::new("first-use-deny".to_string());
    session(&state, "fu-deny");

    let handle = spawn_gate(&state, "fu-deny");
    let card = wait_for_card(&state, "fu-deny");
    answer(&state, "fu-deny", &card, PermissionOutcome::Deny, "deny");
    assert_eq!(
        handle.join().expect("gate thread"),
        Err("permission refused".to_string())
    );
    assert_eq!(state.mcp.first_use_mark("fu-deny", WORKSPACES_GROUP), None);
    assert!(pending_ids(&state, "fu-deny").is_empty());
}

#[test]
fn the_gate_is_per_session() {
    let state = ServerState::new("first-use-per-session".to_string());
    session(&state, "fu-first");
    session(&state, "fu-second");

    let first = spawn_gate(&state, "fu-first");
    let card = wait_for_card(&state, "fu-first");
    answer(
        &state,
        "fu-first",
        &card,
        PermissionOutcome::AllowOnce,
        "allow",
    );
    assert!(first.join().expect("gate thread").is_ok());

    // Another session asks again: its own card, its own answer.
    assert_eq!(
        state.mcp.first_use_mark("fu-second", WORKSPACES_GROUP),
        None
    );
    let second = spawn_gate(&state, "fu-second");
    let card = wait_for_card(&state, "fu-second");
    answer(&state, "fu-second", &card, PermissionOutcome::Deny, "deny");
    assert_eq!(
        second.join().expect("gate thread"),
        Err("permission refused".to_string())
    );
}

#[test]
fn a_session_with_no_live_row_is_refused_without_a_card() {
    let state = ServerState::new("first-use-absent".to_string());
    assert_eq!(
        ensure_write_allowed(&state, &state.mcp, "fu-gone", &owner(), WORKSPACES_GROUP),
        Err("permission refused".to_string())
    );
    assert_eq!(state.mcp.first_use_mark("fu-gone", WORKSPACES_GROUP), None);
}

#[test]
fn forgetting_a_session_clears_its_marks() {
    let state = ServerState::new("first-use-forget".to_string());
    session(&state, "fu-leaver");

    let handle = spawn_gate(&state, "fu-leaver");
    let card = wait_for_card(&state, "fu-leaver");
    answer(
        &state,
        "fu-leaver",
        &card,
        PermissionOutcome::AllowOnce,
        "allow",
    );
    assert!(handle.join().expect("gate thread").is_ok());
    state.mcp.forget_first_use("fu-leaver");
    assert_eq!(
        state.mcp.first_use_mark("fu-leaver", WORKSPACES_GROUP),
        None
    );
}
