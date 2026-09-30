//! What a slash prompt's window publishes: the output between the write and
//! its response reaches the transcript whatever the fate — a losing race, a
//! rejection — and stops reaching it the moment a person has to decide.

use super::local_command_test_support::{
    busy_state, feed, goal_list_notify, notices, notify_of, prompt_response, runtime_mid_turn,
    write_prompt, LocalPi,
};
use devboule_protocol::AgentActivityState;

#[test]
fn a_state_answer_saying_still_streaming_ends_nothing_and_loses_no_output() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut pi = LocalPi::spawn(busy_state());
    let mut reader = pi.reader();
    let (runtime, conn) = runtime_mid_turn();

    write_prompt(&pi, "/goal-list");
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, goal_list_notify());
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, prompt_response("p-1"));
    pi.drain(&mut reader, &runtime, 2);

    assert_eq!(
        runtime.agent_stop_reason(),
        None,
        "a race says the run is still going: nothing ends it"
    );
    assert!(
        matches!(runtime.activity(), AgentActivityState::Working),
        "the roster row stays on the run, got {:?}",
        runtime.activity()
    );
    let output = notices(&conn);
    assert_eq!(
        output.len(),
        1,
        "the windowed output went out with the response: {output:?}"
    );
    assert!(
        output[0].starts_with("No open goals."),
        "a losing race keeps the command's words, got {output:?}"
    );
}

#[test]
fn a_rejected_slash_prompt_still_shows_its_output() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut pi = LocalPi::spawn(busy_state());
    let mut reader = pi.reader();
    let (runtime, conn) = runtime_mid_turn();

    write_prompt(&pi, "/goal-list");
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, goal_list_notify());
    pi.drain(&mut reader, &runtime, 1);
    feed(
        &mut reader,
        &runtime,
        serde_json::json!({
            "id": "p-1",
            "type": "response",
            "command": "prompt",
            "success": false
        }),
    );

    // The rejection ends the run on pi's own words — a rejection means no
    // `turn_end` is coming — and probes nothing, but the words pi wrote
    // between the write and that rejection are still its answer.
    let output = notices(&conn);
    assert_eq!(
        output.len(),
        1,
        "a rejected prompt's output reaches the transcript: {output:?}"
    );
    assert!(output[0].starts_with("No open goals."), "{output:?}");
    assert_eq!(
        runtime.agent_stop_reason().as_deref(),
        Some("error"),
        "a rejected prompt's run is ended, on the journaled road"
    );
    let frames = pi.sent_frames();
    assert!(
        frames.iter().all(|frame| frame["type"] != "get_state"),
        "no probe without a success: {frames:?}"
    );
}

#[test]
fn a_confirm_card_stops_the_window_taking_output() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut pi = LocalPi::spawn(busy_state());
    let mut reader = pi.reader();
    let (runtime, conn) = runtime_mid_turn();

    write_prompt(&pi, "/goal-list");
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, notify_of("before the card"));
    pi.drain(&mut reader, &runtime, 1);
    // The confirm the reader hands to a person: the card it publishes is
    // where the window's collection side closes.
    feed(
        &mut reader,
        &runtime,
        serde_json::json!({
            "type": "extension_ui_request",
            "id": "confirm-1",
            "method": "confirm",
            "title": {
                "title": "Devboule permission",
                "message": "Allow bash?"
            }
        }),
    );
    feed(&mut reader, &runtime, notify_of("while a person decides"));
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, prompt_response("p-1"));
    pi.drain(&mut reader, &runtime, 2);

    let output = notices(&conn);
    assert_eq!(
        output,
        ["before the card"],
        "the person's seconds put nothing more on the transcript: {output:?}"
    );
    assert_eq!(
        runtime.agent_stop_reason(),
        None,
        "the busy answer ends nothing"
    );
}
