//! Tests for the first-use write gate: one human card per session and group,
//! with the two choices the card offers.

use super::*;
use crate::server::ServerState;
use devboule_protocol::{OwnerId, PermissionOutcome, SessionEvent, SessionKind};
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

fn pending_card(state: &Arc<ServerState>, id: &str) -> SessionEvent {
    let card_id = wait_for_card(state, id);
    state
        .sessions
        .live_runtime(id, &owner())
        .expect("live session")
        .permission_broker()
        .expect("test broker")
        .test_pending_request(&card_id)
        .expect("the pending card")
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
            "testing the gate",
            &[("fact", "value")],
        )
    })
}

#[test]
fn the_card_names_both_choices_and_the_facts() {
    let state = ServerState::new("first-use-card".to_string());
    session(&state, "fu-card");

    let handle = spawn_gate(&state, "fu-card");
    let card = pending_card(&state, "fu-card");
    let SessionEvent::PermissionRequest {
        tool_call_id,
        title,
        description,
        options,
        is_chooser,
        ..
    } = card
    else {
        panic!("the gate raises a permission request");
    };
    assert!(
        tool_call_id.starts_with("write:workspaces:fu-card:"),
        "the card names the group and the session: {tool_call_id}"
    );
    assert!(title.contains("testing the gate"), "title: {title}");
    let description = description.expect("description");
    assert!(description.contains("fact: value"), "facts: {description}");
    assert!(
        description.contains("Allow this call") && description.contains("for this session"),
        "both choices: {description}"
    );
    let kinds: Vec<(&str, &str)> = options
        .iter()
        .map(|option| (option.option_id.as_str(), option.kind.as_str()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("once", "allow_once"),
            ("session", "allow_once"),
            ("deny", "reject_once")
        ],
        "a chooser: one kind twice, so the app names both and no agent answers"
    );
    assert_eq!(is_chooser, Some(true), "stamped a chooser at registration");
    answer(
        &state,
        "fu-card",
        &tool_call_id,
        PermissionOutcome::Deny,
        "deny",
    );
    assert!(handle.join().expect("gate thread").is_err());
}

#[test]
fn allow_this_call_proceeds_without_opening_the_group() {
    let state = ServerState::new("first-use-once".to_string());
    session(&state, "fu-once");

    let handle = spawn_gate(&state, "fu-once");
    let card = wait_for_card(&state, "fu-once");
    answer(
        &state,
        "fu-once",
        &card,
        PermissionOutcome::AllowOnce,
        "once",
    );
    assert!(handle.join().expect("gate thread").is_ok());
    assert_eq!(state.mcp.first_use_mark("fu-once", WORKSPACES_GROUP), None);

    // The group stays shut: the next call raises a fresh card.
    let handle = spawn_gate(&state, "fu-once");
    let card = wait_for_card(&state, "fu-once");
    answer(&state, "fu-once", &card, PermissionOutcome::Deny, "deny");
    assert_eq!(
        handle.join().expect("gate thread"),
        Err("permission refused".to_string())
    );
}

#[test]
fn allow_for_this_session_opens_the_group() {
    let state = ServerState::new("first-use-session".to_string());
    session(&state, "fu-session");

    let handle = spawn_gate(&state, "fu-session");
    let card = wait_for_card(&state, "fu-session");
    answer(
        &state,
        "fu-session",
        &card,
        PermissionOutcome::AllowOnce,
        "session",
    );
    assert!(handle.join().expect("gate thread").is_ok());
    assert_eq!(
        state.mcp.first_use_mark("fu-session", WORKSPACES_GROUP),
        Some(GateMark::Open)
    );

    // The second call raises no card: the group stays allowed for the session.
    assert!(ensure_write_allowed(
        &state,
        &state.mcp,
        "fu-session",
        &owner(),
        WORKSPACES_GROUP,
        "testing the gate",
        &[("fact", "value")],
    )
    .is_ok());
    assert!(pending_ids(&state, "fu-session").is_empty());
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
fn a_second_call_racing_the_first_is_pending_not_parked_twice() {
    let state = ServerState::new("first-use-pending".to_string());
    session(&state, "fu-pending");

    let handle = spawn_gate(&state, "fu-pending");
    let card = wait_for_card(&state, "fu-pending");
    assert_eq!(
        ensure_write_allowed(
            &state,
            &state.mcp,
            "fu-pending",
            &owner(),
            WORKSPACES_GROUP,
            "testing the gate",
            &[("fact", "value")],
        ),
        Err("permission pending; retry".to_string())
    );
    assert_eq!(pending_ids(&state, "fu-pending").len(), 1);
    answer(
        &state,
        "fu-pending",
        &card,
        PermissionOutcome::AllowOnce,
        "session",
    );
    assert!(handle.join().expect("gate thread").is_ok());
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
        "session",
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
fn a_delegated_answer_is_refused_like_every_question() {
    let state = ServerState::new("first-use-delegated".to_string());
    session(&state, "fu-creator");
    crate::session::insert_test_child_agent(&state.sessions, "fu-child", owner(), "fu-creator");
    state.delegation.set(true).expect("delegation on");

    let handle = spawn_gate(&state, "fu-child");
    let card = wait_for_card(&state, "fu-child");
    // Even a peer holding answer_permissions cannot open the gate: the card
    // is a chooser, and a chooser stays pending for a person.
    let error = state
        .sessions
        .answer_child_permission("fu-creator", &card, PermissionOutcome::AllowOnce, &|_| {
            vec![crate::peer_policy::CAP_ANSWER_PERMISSIONS.to_string()]
        })
        .expect_err("a gate card is not delegatable");
    assert!(
        error.contains("chooser"),
        "the question rule refuses it: {error}"
    );
    assert_eq!(pending_ids(&state, "fu-child").len(), 1);
    assert_eq!(
        state.mcp.first_use_mark("fu-child", WORKSPACES_GROUP),
        Some(GateMark::Pending),
        "the refusal leaves the gate waiting on the person"
    );

    // The person still can: answering "once" proceeds without opening.
    answer(
        &state,
        "fu-child",
        &card,
        PermissionOutcome::AllowOnce,
        "once",
    );
    assert!(handle.join().expect("gate thread").is_ok());
    assert_eq!(state.mcp.first_use_mark("fu-child", WORKSPACES_GROUP), None);
}

#[test]
fn a_session_with_no_live_row_is_refused_without_a_card() {
    let state = ServerState::new("first-use-absent".to_string());
    assert_eq!(
        ensure_write_allowed(
            &state,
            &state.mcp,
            "fu-gone",
            &owner(),
            WORKSPACES_GROUP,
            "testing the gate",
            &[("fact", "value")],
        ),
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
        "session",
    );
    assert!(handle.join().expect("gate thread").is_ok());
    state.mcp.forget_first_use("fu-leaver");
    assert_eq!(
        state.mcp.first_use_mark("fu-leaver", WORKSPACES_GROUP),
        None
    );
}
