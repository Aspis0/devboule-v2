//! Tests for one topic: the Claude unread-steer latch. A person-borne steer
//! denies the CLI's next permission requests outright — the steer is the
//! answer — until the measured `command_lifecycle` frames say the steer was
//! read or retired; turn end and interrupt clear it; a host carded tool and
//! an auto-answered mode are never touched. The latch's own release rule is
//! unit-tested here too, against the measured frame states.

use super::steer_latch_test_support::{
    can_use_tool, deny_answers, drain, feed, latch_harness, latch_harness_echo, latch_parts,
    lifecycle, published_cards, read_echoed_line, release_gate, LatchParts,
};
use super::*;
use std::time::Duration;

#[test]
fn a_lifecycle_frame_releases_the_uuid_for_every_state_but_queued() {
    let latch = ClaudeSteerLatch::default();
    latch.arm("u1");
    latch.arm("u2");
    latch.release_read(&serde_json::json!({
        "type": "command_lifecycle", "command_uuid": "u1", "state": "started",
    }));
    assert!(latch.is_armed(), "u2 is still unread");
    latch.release_read(&serde_json::json!({
        "type": "command_lifecycle", "command_uuid": "u2", "state": "queued",
    }));
    assert!(
        latch.is_armed(),
        "queued is the buffer ack, not a read or a retirement"
    );
    // Measured on kill: the CLI saying the command is gone and will never be
    // read is exactly the condition under which nothing is superseded.
    latch.release_read(&serde_json::json!({
        "type": "command_lifecycle", "command_uuid": "u2", "state": "cancelled",
    }));
    assert!(!latch.is_armed(), "a cancelled command can never be read");
    latch.arm("u3");
    latch.release_read(&serde_json::json!({
        "type": "command_lifecycle", "command_uuid": "u3", "state": "completed",
    }));
    assert!(!latch.is_armed(), "every tracked uuid was retired");
}

#[test]
fn a_non_lifecycle_frame_releases_nothing() {
    let latch = ClaudeSteerLatch::default();
    latch.arm("u1");
    latch.release_read(&serde_json::json!({
        "type": "assistant", "message": {"role": "assistant", "content": []},
    }));
    latch.release_read(&serde_json::json!({
        "type": "command_lifecycle", "state": "started",
    }));
    latch.release_read(&serde_json::json!({
        "type": "command_lifecycle", "command_uuid": "u1", "state": "queued",
    }));
    assert!(
        latch.is_armed(),
        "only the steer's own lifecycle frames release it"
    );
}

#[test]
fn clear_empties_the_latch_whatever_was_tracked() {
    let latch = ClaudeSteerLatch::default();
    latch.arm("u1");
    latch.arm("u2");
    latch.clear();
    assert!(!latch.is_armed());
}

#[test]
fn a_person_steer_denies_the_next_permission_until_the_cli_reads_it() {
    let mut harness = latch_harness();
    harness.latch.arm("steer-1");
    feed(
        &mut harness.reader,
        &harness.runtime,
        can_use_tool("req-1", "Write"),
    );
    let events = drain(&harness.conn);
    assert!(
        published_cards(&events).is_empty(),
        "the superseded request never becomes a card: {events:?}"
    );
    assert!(
        !events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::PermissionResolved { .. } | SessionEvent::PermissionAnswered { .. }
            )
        }),
        "no resolution or answered row is published for a card nobody was shown: {events:?}"
    );
    assert_eq!(harness.broker.pending_len(), 0, "nothing stays pending");
    assert!(
        harness.runtime.permission_already_recorded("toolu_req-1"),
        "the journal holds the denial — the one record a card-less deny leaves"
    );
    let denied = deny_answers(&harness);
    assert_eq!(denied.len(), 1, "exactly one deny went out");
    assert_eq!(
        denied[0]
            .pointer("/outcome/message")
            .and_then(Value::as_str),
        Some(STEER_SUPERSEDED_MESSAGE),
        "the CLI is told the steer is the answer: journal degraded = {}, capture = {}",
        harness.journal_is_degraded(),
        denied[0]
    );
    let written = harness.written.lock().expect("written");
    let frame = written
        .iter()
        .find(|line| line.contains("\"req-1\""))
        .expect("the deny control_response was written");
    assert!(
        frame.contains("\"behavior\":\"deny\"") && frame.contains(STEER_SUPERSEDED_MESSAGE),
        "the written frame denies and carries the sentence, end to end: {frame}"
    );
}

#[test]
fn a_lifecycle_started_frame_releases_the_latch_and_a_queued_frame_does_not() {
    let mut harness = latch_harness();
    harness.latch.arm("steer-1");
    // The measured buffer ack, sent while the CLI is still parked: no release.
    feed(
        &mut harness.reader,
        &harness.runtime,
        lifecycle("steer-1", "queued"),
    );
    feed(
        &mut harness.reader,
        &harness.runtime,
        can_use_tool("req-queued", "Write"),
    );
    let held = drain(&harness.conn);
    assert!(
        published_cards(&held).is_empty(),
        "queued does not read the steer; the request stays superseded: {held:?}"
    );
    // The measured read: the steer's own uuid, started.
    feed(
        &mut harness.reader,
        &harness.runtime,
        lifecycle("steer-1", "started"),
    );
    feed(
        &mut harness.reader,
        &harness.runtime,
        can_use_tool("req-after", "Bash"),
    );
    let released = drain(&harness.conn);
    assert_eq!(
        published_cards(&released),
        vec!["toolu_req-after".to_string()],
        "a permission the agent asks for after the read reaches the person: {released:?}"
    );
    assert_eq!(
        deny_answers(&harness).len(),
        1,
        "only the pre-release request was denied"
    );
}

#[test]
fn the_latch_is_cleared_when_the_turn_ends_and_when_interrupted() {
    let mut harness = latch_harness();
    harness.runtime.begin_turn();
    harness.latch.arm("steer-1");
    feed(
        &mut harness.reader,
        &harness.runtime,
        can_use_tool("req-in-turn", "Write"),
    );
    let in_turn = drain(&harness.conn);
    assert!(
        published_cards(&in_turn).is_empty(),
        "the control half: armed means superseded: {in_turn:?}"
    );
    // The turn's result arrives: the unread window is over, whatever the CLI
    // reported about the steer itself.
    feed(
        &mut harness.reader,
        &harness.runtime,
        serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": "done",
        }),
    );
    let _ = drain(&harness.conn);
    assert!(!harness.latch.is_armed(), "a turn end clears the latch");
    feed(
        &mut harness.reader,
        &harness.runtime,
        can_use_tool("req-next-turn", "Bash"),
    );
    let next_turn = drain(&harness.conn);
    assert_eq!(
        published_cards(&next_turn),
        vec!["toolu_req-next-turn".to_string()],
        "a request in the next turn is a normal card: {next_turn:?}"
    );

    // Interrupt takes the shared soft-interrupt road: armed again, the
    // interrupt clears the latch so nothing outlives the turn it armed in.
    harness.latch.arm("steer-2");
    let stdin: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(None));
    interrupt_claude_turn(
        &stdin,
        &Arc::new(AtomicU64::new(1)),
        &harness.broker,
        &Arc::new(crate::claude_abort::ClaudeAbortGate::default()),
        &Arc::new(AtomicBool::new(false)),
        &harness.latch,
    );
    assert!(!harness.latch.is_armed(), "an interrupt clears the latch");
}

/// The interleave the result arm must close: the steer's arm runs under the
/// turn lock, so the latch may only be emptied after `settle_turn_finish`
/// has answered the turn question under that same lock. Here the reader is
/// held in the settle (the test owns the turn lock while it feeds the
/// result) and the arm lands while it waits — the clear that follows the
/// settle must retire it.
#[test]
fn a_result_settles_the_turn_before_the_latch_is_cleared() {
    let LatchParts {
        mut reader,
        latch,
        runtime,
        conn,
        ..
    } = latch_parts();
    runtime.begin_turn();
    let expected = runtime.turn_counter();
    // The feeder starts while the lock is held, so its reader can only block
    // in the settle; the join happens after the closure releases the lock, or
    // the settle would wait on a lock the join is blocking on.
    let feeder = runtime
        .with_active_turn(expected, |_turn| {
            let feeder_runtime = Arc::clone(&runtime);
            let mut feeder_reader = std::mem::replace(&mut reader, orphan_reader(&runtime, &latch));
            let feeder = std::thread::spawn(move || {
                feed(
                    &mut feeder_reader,
                    &feeder_runtime,
                    serde_json::json!({
                        "type": "result",
                        "subtype": "success",
                        "is_error": false,
                        "result": "done",
                    }),
                );
            });
            // The reader is now blocked in the settle. Give it that long; the
            // arm below is the steer admitted under the lock in that window.
            std::thread::sleep(Duration::from_millis(400));
            latch.arm("steer-race");
            feeder
        })
        .expect("the turn is running");
    feeder.join().expect("the reader thread finishes");
    let _ = drain(&conn);
    assert!(
        !latch.is_armed(),
        "a steer admitted under the turn lock dies with the turn the result settled"
    );
}

#[test]
fn an_auto_answered_mode_is_not_denied_while_the_latch_is_armed() {
    let mut harness = latch_harness();
    harness
        .runtime
        .set_agent_kind(devboule_protocol::SessionKind::Claude);
    harness.latch.arm("steer-1");
    // The CLI reports itself in bypassPermissions: the manifest says the
    // mode auto-answers, and a latch deny there would refuse a request the
    // person's own mode was about to grant.
    feed(
        &mut harness.reader,
        &harness.runtime,
        serde_json::json!({
            "type": "system",
            "subtype": "init",
            "cwd": "C:\\tmp",
            "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd",
            "tools": ["Bash"],
            "model": "claude-opus-5[1m]",
            "permissionMode": "bypassPermissions",
            "claude_code_version": "2.1.260",
        }),
    );
    feed(
        &mut harness.reader,
        &harness.runtime,
        can_use_tool("req-bypass", "Bash"),
    );
    let _ = drain(&harness.conn);
    assert!(
        deny_answers(&harness).is_empty(),
        "an auto-answered mode is not overridden by the latch"
    );
    let written = harness.written.lock().expect("written");
    let frame = written
        .iter()
        .find(|line| line.contains("\"req-bypass\""))
        .expect("the auto-allow control_response was written");
    assert!(
        frame.contains("\"behavior\":\"allow\""),
        "the request was answered the way the mode answers it: {frame}"
    );
}

#[test]
fn a_mode_switch_clears_the_latch() {
    let mut harness = latch_harness_echo();
    release_gate(&mut harness);
    // The manifest gives the switch's confirmation somewhere to report the
    // mode; without it a confirmed switch is refused by the reporter, not
    // by anything this test is about.
    feed(
        &mut harness.reader,
        &harness.runtime,
        serde_json::json!({
            "type": "system",
            "subtype": "init",
            "cwd": "C:\tmp",
            "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd",
            "tools": ["Bash"],
            "model": "claude-opus-5[1m]",
            "permissionMode": "default",
            "claude_code_version": "2.1.260",
        }),
    );
    harness.latch.arm("steer-1");
    // The switcher waits on the map the reader consults, or its answer is
    // dropped as unknown.
    let switcher = ClaudeSwitcher {
        stdin: Arc::clone(&harness.stdin),
        next_id: Arc::new(AtomicU64::new(2)),
        mode_responses: Arc::clone(&harness.mode_responses),
        mode_gate: Some(Arc::clone(&harness.mode_gate)),
        abort_gate: Arc::new(crate::claude_abort::ClaudeAbortGate::default()),
        steer_latch: Arc::clone(&harness.latch),
    };
    let switched = std::thread::spawn(move || {
        switcher
            .set_mode("bypassPermissions")
            .expect("the mode switch is confirmed");
    });
    let request = read_echoed_line(harness.child_as_mut());
    feed(
        &mut harness.reader,
        &harness.runtime,
        serde_json::json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request["request_id"],
                "response": {"mode": "bypassPermissions"},
            },
        }),
    );
    switched.join().expect("the mode switch completes");
    assert!(
        !harness.latch.is_armed(),
        "the mode the person chose is not silently overridden by an unread steer"
    );
}

#[test]
fn an_agent_steer_arms_nothing_but_a_person_steer_arms_the_latch() {
    let mut harness = latch_harness_echo();
    release_gate(&mut harness);
    harness.runtime.begin_turn();
    let expected = harness.runtime.turn_counter();
    let mut steerer = ClaudeSteerer {
        stdin: Arc::clone(&harness.stdin),
        mode_gate: Some(Arc::clone(&harness.mode_gate)),
        abort_gate: Arc::new(crate::claude_abort::ClaudeAbortGate::default()),
        steer_latch: Arc::clone(&harness.latch),
    };
    let agent = harness
        .runtime
        .with_active_turn(expected, |turn| {
            steerer.steer_active_turn("from another agent", turn, SteerOrigin::Agent)
        })
        .expect("the turn is running")
        .expect("the agent steer was written");
    assert!(agent);
    assert!(
        !harness.latch.is_armed(),
        "an agent-to-agent steer supersedes nothing"
    );
    let person = harness
        .runtime
        .with_active_turn(expected, |turn| {
            steerer.steer_active_turn("from the person", turn, SteerOrigin::Person)
        })
        .expect("the turn is running")
        .expect("the person steer was written");
    assert!(person);
    assert!(
        harness.latch.is_armed(),
        "a person-borne steer arms the latch"
    );
}

#[test]
fn a_host_carded_tool_is_never_denied_while_the_latch_is_armed() {
    let mut harness = latch_harness();
    harness.latch.arm("steer-1");
    // The control half: a Claude permission request under the same latch is
    // superseded, so this test can tell the host card's survival apart from
    // a latch that never fired.
    feed(
        &mut harness.reader,
        &harness.runtime,
        can_use_tool("req-agent", "Write"),
    );
    assert!(
        published_cards(&drain(&harness.conn)).is_empty(),
        "the control half: armed means the agent's request is superseded"
    );
    harness
        .broker
        .register_host_plan(host_plan_card("host-plan-1"), &harness.runtime)
        .expect("the host card registers");
    let events = drain(&harness.conn);
    assert_eq!(
        published_cards(&events),
        vec!["host-plan-1".to_string()],
        "a host carded tool is published as a normal card: {events:?}"
    );
    assert_eq!(
        deny_answers(&harness).len(),
        1,
        "the deny that went out was the agent request's, never the host card's"
    );
    assert_eq!(harness.broker.pending_len(), 1, "the host card stays open");
}

#[test]
fn a_latch_denied_plan_keeps_its_cancelled_plan_row_live_and_in_replay() {
    let mut harness = latch_harness();
    harness.latch.arm("steer-1");
    feed(
        &mut harness.reader,
        &harness.runtime,
        serde_json::json!({
            "type": "control_request",
            "request_id": "req-plan",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "ExitPlanMode",
                "display_name": "ExitPlanMode",
                "input": {"plan": "the plan text"},
                "tool_use_id": "toolu_req-plan",
            }
        }),
    );
    let events = drain(&harness.conn);
    assert!(
        published_cards(&events).is_empty(),
        "the superseded plan never becomes a card: {events:?}"
    );
    assert_eq!(deny_answers(&harness).len(), 1, "exactly one deny went out");
    let terminal = events.iter().find(|event| {
        matches!(
            event,
            SessionEvent::AgentToolUpdate {
                kind: Some(kind),
                ..
            } if kind == "plan"
        )
    });
    assert_eq!(
        terminal,
        Some(&SessionEvent::AgentToolUpdate {
            tool_call_id: "toolu_req-plan".to_string(),
            status: Some("cancelled".to_string()),
            text: None,
            title: Some(
                "The user answered with a message instead of approving the plan.".to_string()
            ),
            kind: Some("plan".to_string()),
            locations: None,
            parent_tool_use_id: None,
            spawn_depth: None,
            command: None,
            exit_code: None,
            replace: false,
        }),
        "the plan row's terminal state is the daemon's, live: {events:?}"
    );
    // And durable: a restart replays the same row.
    let replayed = harness.replay_events();
    assert!(
        replayed.iter().any(|event| matches!(
            event,
            SessionEvent::AgentToolUpdate {
                kind: Some(kind),
                status: Some(status),
                ..
            } if kind == "plan" && status == "cancelled"
        )),
        "the cancelled plan row survives replay: {replayed:?}"
    );
}

/// A throwaway reader for a test that must move the real one to a thread.
fn orphan_reader(runtime: &Arc<SessionRuntime>, latch: &Arc<ClaudeSteerLatch>) -> ClaudeReader {
    let abort_gate: ClaudeAbortGateRef = Arc::new(crate::claude_abort::ClaudeAbortGate::default());
    let mut wiring = ClaudeModeGateWiring::new(
        Arc::new(Mutex::new(None)),
        Arc::new(Mutex::new(ClaudeModeGate {
            state: ClaudeModeGateState::Ready,
            pending_frames: Vec::new(),
        })),
        abort_gate,
    );
    wiring.steer_latch = Arc::clone(latch);
    let _ = runtime;
    ClaudeReader::with_mode_gate(
        ClaudeView::new(None),
        PermissionBroker::for_test(Arc::new(|_, _| Ok(()))),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(AtomicU64::new(1)),
        wiring,
        None,
    )
}

fn host_plan_card(tool_call_id: &str) -> SessionEvent {
    SessionEvent::PermissionRequest {
        tool_call_id: tool_call_id.to_string(),
        title: "Plan".to_string(),
        description: None,
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![
            devboule_protocol::PermissionOption {
                option_id: "deny".to_string(),
                name: "Reject".to_string(),
                kind: "reject_once".to_string(),
            },
            devboule_protocol::PermissionOption {
                option_id: "implement".to_string(),
                name: "Implement".to_string(),
                kind: "allow_once".to_string(),
            },
        ],
        is_chooser: None,
        kind: Some(devboule_protocol::PermissionRequestKind::Plan),
        plan: Some("the plan text".to_string()),
        questions: None,
        origin: devboule_protocol::SessionOrigin::local(),
        create_agent: None,
    }
}
