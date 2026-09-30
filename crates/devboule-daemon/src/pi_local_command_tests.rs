//! The local-slash-command cases: a slash prompt Pi answers without a model
//! turn must still end its run, its output shown. The frames are the ones
//! measured on a live `pi --mode rpc` (`/goal-list` beside an ordinary
//! prompt): notify, then the prompt response, then nothing — no
//! `agent_start`, no `turn_end`.

use super::local_command_test_support::{
    agent_start, feed, finishes, goal_list_notify, idle_state, notices, prompt_response,
    recorded_turn_end, runtime_mid_turn, write_prompt, LocalPi,
};
use devboule_protocol::AgentActivityState;

#[test]
fn a_locally_handled_slash_command_shows_its_output_and_ends_its_run() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut pi = LocalPi::spawn(idle_state());
    let mut reader = pi.reader();
    let (runtime, conn) = runtime_mid_turn();

    // The write, then the measured frames: the notify between the write and
    // its response, then the response — and nothing else ever comes.
    write_prompt(&pi, "/goal-list");
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, goal_list_notify());
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, prompt_response("p-1"));
    pi.drain(&mut reader, &runtime, 2);

    // The run is over on the journaled finish path, idle, and the command's
    // output is on the transcript. One pull carries it once.
    assert_eq!(
        runtime.agent_stop_reason().as_deref(),
        Some("completed"),
        "a locally handled command ends its run"
    );
    assert!(
        matches!(runtime.activity(), AgentActivityState::Idle),
        "the roster row returns to idle, got {:?}",
        runtime.activity()
    );
    let output = notices(&conn);
    assert_eq!(output.len(), 1, "the notify, once, as the command's output");
    assert!(
        output[0].starts_with("No open goals."),
        "the output is pi's own words, got {output:?}"
    );

    // A notify after the run ended belongs to no run: dropped, not a second
    // output line.
    feed(&mut reader, &runtime, goal_list_notify());
    pi.drain(&mut reader, &runtime, 1);
    assert!(
        notices(&conn).is_empty(),
        "the late notify is not attributed to the ended run"
    );

    // The decision was Pi's own answer: exactly one get_state went out.
    let frames = pi.sent_frames();
    assert_eq!(
        frames.iter().filter(|f| f["type"] == "get_state").count(),
        1,
        "one get_state decides the fate: {frames:?}"
    );
}

#[test]
fn an_ordinary_prompt_ends_as_today_and_never_asks_for_state() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut pi = LocalPi::spawn(idle_state());
    let mut reader = pi.reader();
    let (runtime, conn) = runtime_mid_turn();

    write_prompt(&pi, "Reply with the single word OK.");
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, prompt_response("p-1"));
    feed(&mut reader, &runtime, agent_start());
    feed(&mut reader, &runtime, recorded_turn_end());

    assert_eq!(
        runtime.agent_stop_reason().as_deref(),
        Some("stop"),
        "the model turn ends the run exactly as before"
    );
    assert!(notices(&conn).is_empty(), "no notify, no output line");

    // Neither a POSIX path pasted as a prompt nor the compact the guard
    // owns may reach pi with a probe behind them.
    write_prompt(&pi, "/home/gualt/devboule-v2 what changed?");
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, prompt_response("p-2"));
    write_prompt(&pi, "/compact");
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, prompt_response("p-3"));

    let frames = pi.sent_frames();
    assert_eq!(
        frames,
        [
            serde_json::json!({"id":"p-1","type":"prompt","message":"Reply with the single word OK."}),
            serde_json::json!({"id":"p-2","type":"prompt","message":"/home/gualt/devboule-v2 what changed?"}),
            serde_json::json!({"id":"p-3","type":"prompt","message":"/compact"}),
        ],
        "the writer's bytes, and no probe for any of the three prompts"
    );
}

#[test]
fn a_turn_starting_before_the_state_answer_ends_nothing_early() {
    // The order pi is believed to write for a slash command that starts a
    // turn — `agent_start` with the response, ahead of any answer — mirrors
    // the measured ordinary prompt. The answer arriving first is a case of
    // its own, `a_state_answer_before_the_turn_starts...`.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut pi = LocalPi::spawn(idle_state());
    let mut reader = pi.reader();
    let (runtime, _conn) = runtime_mid_turn();

    write_prompt(&pi, "/review everything");
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, prompt_response("p-1"));
    // The probe is already on the wire when the turn starts; its answer
    // must decide nothing.
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, agent_start());
    pi.drain(&mut reader, &runtime, 1);
    assert_eq!(
        runtime.agent_stop_reason(),
        None,
        "a started turn is not ended by the state answer"
    );
    feed(&mut reader, &runtime, recorded_turn_end());
    assert_eq!(
        runtime.agent_stop_reason().as_deref(),
        Some("stop"),
        "the turn ends the way a model turn always has"
    );
    let frames = pi.sent_frames();
    assert_eq!(
        frames.iter().filter(|f| f["type"] == "get_state").count(),
        1,
        "the probe went out before the turn showed: {frames:?}"
    );
}

#[test]
fn a_state_answer_before_the_turn_starts_ends_the_command_then_the_turn() {
    // The unmeasured order: pi answers the probe idle, then starts the
    // turn anyway. The command ends — and when the turn shows up after it,
    // the reader begins the turn the send path never did, so pi's own
    // turn_end ends it exactly once on a truthful roster.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut pi = LocalPi::spawn(idle_state());
    let mut reader = pi.reader();
    let (runtime, conn) = runtime_mid_turn();

    write_prompt(&pi, "/review everything");
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, prompt_response("p-1"));
    pi.drain(&mut reader, &runtime, 2);

    assert_eq!(
        runtime.agent_stop_reason().as_deref(),
        Some("completed"),
        "the idle answer ends the command's run"
    );
    feed(&mut reader, &runtime, agent_start());
    assert!(
        matches!(runtime.activity(), AgentActivityState::Working),
        "the turn that shows up after the end re-arms the roster, got {:?}",
        runtime.activity()
    );
    feed(&mut reader, &runtime, recorded_turn_end());
    assert_eq!(
        runtime.agent_stop_reason().as_deref(),
        Some("stop"),
        "the turn ends once, on its own turn_end"
    );
    assert!(
        matches!(runtime.activity(), AgentActivityState::Idle),
        "and the roster rests, got {:?}",
        runtime.activity()
    );
    assert_eq!(
        finishes(&conn),
        ["completed", "stop"],
        "the wire carries each ending once: the command's, then the turn's"
    );
}

#[test]
fn a_follow_up_prompt_moots_the_stale_probe_answer() {
    // An inter-agent submit lands while the probe is in flight: the new
    // prompt's write mutes it, and its stale idle answer must end neither
    // the new turn nor anything else.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut pi = LocalPi::spawn(idle_state());
    let mut reader = pi.reader();
    let (runtime, conn) = runtime_mid_turn();

    write_prompt(&pi, "/goal-list");
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, prompt_response("p-1"));
    pi.drain(&mut reader, &runtime, 1);

    // The follow-up the send path runs: its write, then its begin_turn.
    write_prompt(&pi, "status report");
    runtime.begin_turn();

    pi.drain(&mut reader, &runtime, 1);
    assert_eq!(
        runtime.agent_stop_reason(),
        None,
        "a stale probe answer is not a finish"
    );
    assert!(
        matches!(runtime.activity(), AgentActivityState::Working),
        "the follow-up's turn is untouched, got {:?}",
        runtime.activity()
    );
    feed(&mut reader, &runtime, agent_start());
    feed(&mut reader, &runtime, recorded_turn_end());

    assert_eq!(
        finishes(&conn),
        ["stop"],
        "the new turn ends exactly once, on its own turn_end"
    );
    assert_eq!(
        runtime.agent_stop_reason().as_deref(),
        Some("stop"),
        "and the roster rests on that ending"
    );
}
