//! A mode switch on a running Claude child reaches the live CLI, and the card
//! that was already waiting: against the stub CLI.
//!
//! The stub asks for permission on an `EDIT` prompt while its mode asks, and
//! approves edits itself once the mode is `acceptEdits`, the way the CLI does.

#![cfg(windows)]

#[path = "claude_common/mod.rs"]
mod common;

#[path = "scratch_workspace/mod.rs"]
mod scratch_workspace;
use scratch_workspace::scratch_workspace;

use std::time::Duration;

use devboule_protocol::{SessionEvent, SessionKind};

fn card_count(events: &[SessionEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, SessionEvent::PermissionRequest { .. }))
        .count()
}

fn says(events: &[SessionEvent], text: &str) -> bool {
    events.iter().any(|event| {
        matches!(event, SessionEvent::AgentMessage { text: said, .. } if said.contains(text))
    })
}

#[test]
fn a_mode_switch_answers_the_waiting_card_and_the_next_edit_raises_none() {
    let _test_lock = common::lock_tests();
    let observation = devboule_daemon::test_dirs::test_temp_dir("devboule-claude-mode-switch");
    std::fs::create_dir_all(&observation).expect("observation dir");
    let argv_file = observation.join("argv.txt");
    let console_file = observation.join("console.txt");
    let home = observation.join("home");
    std::fs::create_dir_all(&home).expect("fake home");
    let _env = common::use_stub_cli(&argv_file, &console_file, &home);

    let harness = common::Harness::spawn();
    let client = std::sync::Arc::new(harness.client());
    let session = client
        .session_create(Some(scratch_workspace(&client)), SessionKind::Claude, None)
        .expect("create Claude session");
    let (events, handler) = common::collect_events();
    client
        .session_attach(&session.id, None, handler)
        .expect("attach Claude session");
    common::wait_for(&events, Duration::from_secs(15), "manifest", |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::SessionManifest { .. }))
    });

    client
        .session_send(&session.id, "EDIT the first note")
        .expect("first edit");
    common::wait_for(
        &events,
        Duration::from_secs(15),
        "the first card",
        |events| card_count(events) == 1,
    );

    client
        .session_set_mode(&session.id, "acceptEdits")
        .expect("switch to Allow edits while the card waits");
    common::wait_for(
        &events,
        Duration::from_secs(15),
        "the card answered",
        |events| says(events, "STUB-EDIT-DONE:asked"),
    );

    client
        .session_send(&session.id, "EDIT the second note")
        .expect("second edit");
    common::wait_for(
        &events,
        Duration::from_secs(15),
        "the second edit",
        |events| says(events, "STUB-EDIT-DONE:auto"),
    );
    let events = events.lock().expect("events");
    assert_eq!(
        card_count(&events),
        1,
        "the edit after the switch raised no card"
    );
    drop(events);

    client
        .session_close(&session.id)
        .expect("close Claude session");
    let _ = std::fs::remove_dir_all(&observation);
}
