//! The interrupt race against the stub CLI: a stale aborted result must not
//! finish a run a replacement already joined, an interrupted turn must end
//! as interrupted, and a reattach must not replay a finish the live run
//! suppressed. The stub replays the abort sequence measured from the live
//! CLI (`fixtures/claude-aborted-result.json`), in either ordering: result
//! before the replacement (`HOLD`) or after it (`HOLD-AFTER`, the stub holds
//! the result until it has read the replacement line).

#![cfg(windows)]

#[path = "claude_common/mod.rs"]
mod common;

use std::time::Duration;

use devboule_protocol::{SessionEvent, SessionKind};

fn finished_stop_reasons(events: &std::sync::MutexGuard<'_, Vec<SessionEvent>>) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentFinished { stop_reason, .. } => Some(stop_reason.clone()),
            _ => None,
        })
        .collect()
}

fn is_agent_message(events: &[SessionEvent]) -> bool {
    events.iter().any(|event| {
        matches!(event, SessionEvent::AgentMessage { .. })
            || matches!(event, SessionEvent::AgentToolCall { .. })
    })
}

fn is_finish(events: &[SessionEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, SessionEvent::AgentFinished { .. }))
}

struct InterruptEnv {
    _env: common::EnvGuard,
    _harness: common::Harness,
    events: std::sync::Arc<std::sync::Mutex<Vec<SessionEvent>>>,
    client: std::sync::Arc<devboule_daemon::DaemonClient>,
    session_id: String,
    console_file: std::path::PathBuf,
    // Declared last on purpose: fields drop in declaration order, and the lock
    // must outlive the env restore. Dropped first, it would release the next
    // test into `use_stub_cli` before this one's guard removed the command it
    // had just set, and that test's daemon would start without the stub CLI.
    _test_lock: std::sync::MutexGuard<'static, ()>,
}

fn setup(name: &str) -> InterruptEnv {
    let _test_lock = common::lock_tests();
    let observation = std::env::temp_dir().join(format!(
        "devboule-claude-interrupt-{}-{name}",
        std::process::id()
    ));
    std::fs::create_dir_all(&observation).expect("observation dir");
    let console_file = observation.join("console.txt");
    let home = observation.join("home");
    std::fs::create_dir_all(&home).expect("fake home");
    let _env = common::use_stub_cli(&observation.join("argv.txt"), &console_file, &home);

    let harness = common::Harness::spawn();
    let client = std::sync::Arc::new(harness.client());
    let session = client
        .session_create(None, SessionKind::Claude, None)
        .expect("create Claude session");
    let (events, handler) = common::collect_events();
    client
        .session_attach(&session.id, None, handler)
        .expect("attach Claude session");
    common::wait_for(&events, Duration::from_secs(15), "manifest", |seen| {
        seen.iter()
            .any(|event| matches!(event, SessionEvent::SessionManifest { .. }))
    });
    InterruptEnv {
        _test_lock,
        _env,
        _harness: harness,
        events,
        client,
        session_id: session.id,
        console_file,
    }
}

/// The stub has read the replacement once its console names it; in
/// `HOLD-AFTER` mode that is also the moment the deferred aborted result
/// starts existing on stdout, so the daemon's admission of the replacement
/// necessarily precedes its read of the result.
fn wait_console_contains(console_file: &std::path::Path, needle: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if std::fs::read_to_string(console_file)
            .unwrap_or_default()
            .contains(needle)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the stub never read the {needle:?} prompt");
}

/// The race the fix exists for, forced by the fixture: the replacement is
/// admitted (written to the CLI) before the aborted result exists, and the
/// stale result must not finish the run it aborted. Only the replacement's
/// own result completes it — one finish, `end_turn`. The ordering is the
/// turn-hold's, not the fixture's: `with_active_turn` holds it across the
/// steer write, so the count lands inside it and the reader's settle cannot
/// pass — simplify that lock and this test re-flakes silently.
#[test]
fn stale_aborted_result_does_not_finish_the_replacement_run() {
    let env = setup("stale-abort");
    env.client
        .session_send(&env.session_id, "HOLD-AFTER: slow tool call")
        .expect("held prompt");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "held echo",
        is_agent_message,
    );

    env.client
        .session_interrupt(&env.session_id)
        .expect("interrupt");
    env.client
        .session_send(&env.session_id, "Reply now")
        .expect("replacement prompt");
    wait_console_contains(&env.console_file, "Reply now");

    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "replacement result",
        is_finish,
    );
    let reasons = finished_stop_reasons(&env.events.lock().expect("events lock"));
    assert_eq!(reasons, vec!["end_turn".to_string()]);
}

/// The other ordering: the aborted result arrives with no replacement
/// admitted, and that is a legitimate end — the interrupted run finishes as
/// interrupted, the replacement sent afterwards is a NEW run with its own
/// completion. Two runs, two finishes, in order.
#[test]
fn aborted_result_before_the_replacement_ends_that_run_and_the_replacement_runs_anew() {
    let env = setup("abort-first");
    env.client
        .session_send(&env.session_id, "HOLD: slow tool call")
        .expect("held prompt");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "held echo",
        is_agent_message,
    );

    env.client
        .session_interrupt(&env.session_id)
        .expect("interrupt");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "interrupted finish",
        is_finish,
    );
    assert_eq!(
        finished_stop_reasons(&env.events.lock().expect("events lock")),
        vec!["interrupted".to_string()]
    );

    env.client
        .session_send(&env.session_id, "Reply now")
        .expect("replacement as a new run");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "second finish",
        |seen| {
            seen.iter()
                .filter(|event| matches!(event, SessionEvent::AgentFinished { .. }))
                .count()
                == 2
        },
    );
    assert_eq!(
        finished_stop_reasons(&env.events.lock().expect("events lock")),
        vec!["interrupted".to_string(), "end_turn".to_string()]
    );
}

/// A reattach replays the journal: the withheld aborted result must not
/// come back as an interrupted finish in the middle of the replacement's
/// transcript. The journal keeps the wire frame, so what replay reads is
/// the withholding marker the live pass wrote ahead of it.
#[test]
fn replay_after_a_withheld_abort_shows_one_finish() {
    let env = setup("replay");
    env.client
        .session_send(&env.session_id, "HOLD-AFTER: slow tool call")
        .expect("held prompt");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "held echo",
        is_agent_message,
    );
    env.client
        .session_interrupt(&env.session_id)
        .expect("interrupt");
    env.client
        .session_send(&env.session_id, "Reply now")
        .expect("replacement prompt");
    wait_console_contains(&env.console_file, "Reply now");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "replacement result",
        is_finish,
    );
    assert_eq!(
        finished_stop_reasons(&env.events.lock().expect("events lock")),
        vec!["end_turn".to_string()]
    );

    let replay_client = env._harness.client_named("replay");
    let (replayed, handler) = common::collect_events();
    replay_client
        .session_attach(&env.session_id, None, handler)
        .expect("reattach");
    common::wait_for(
        &replayed,
        Duration::from_secs(15),
        "replayed finish",
        is_finish,
    );
    let replayed = replayed.lock().expect("replayed events lock");
    assert_eq!(
        finished_stop_reasons(&replayed),
        vec!["end_turn".to_string()]
    );
}

/// An interrupt with no turn running must not arm a suppression: armed, it
/// would swallow the only completion of the NEXT run. Idle stop, then a held
/// turn, then a real stop: the single aborted result ends the run once.
#[test]
fn idle_interrupt_then_a_live_one_finishes_the_run_on_its_single_result() {
    let env = setup("idle-then-live");
    env.client
        .session_interrupt(&env.session_id)
        .expect("idle interrupt");

    env.client
        .session_send(&env.session_id, "HOLD: slow tool call")
        .expect("held prompt");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "held echo",
        is_agent_message,
    );

    env.client
        .session_interrupt(&env.session_id)
        .expect("live interrupt");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "interrupted finish",
        is_finish,
    );
    let reasons = finished_stop_reasons(&env.events.lock().expect("events lock"));
    assert_eq!(reasons, vec!["interrupted".to_string()]);
}

/// Interrupt, steer the replacement into the still-running turn, interrupt
/// again: the CLI answers with ONE aborted result for the one turn, and the
/// run must end on it exactly once — the second stop re-baselines the
/// expectation onto the count that already includes the replacement.
#[test]
fn interrupt_steered_interrupt_one_result_finishes_once() {
    let env = setup("double-interrupt");
    env.client
        .session_send(&env.session_id, "HOLD-SILENT: first")
        .expect("first held prompt");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "first echo",
        is_agent_message,
    );

    env.client
        .session_interrupt(&env.session_id)
        .expect("first interrupt (no result follows)");
    env.client
        .session_send(&env.session_id, "HOLD: second")
        .expect("steered replacement, itself held");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "second echo",
        |seen| {
            seen.iter()
                .filter(|event| matches!(event, SessionEvent::AgentMessage { .. }))
                .count()
                >= 2
        },
    );

    env.client
        .session_interrupt(&env.session_id)
        .expect("second interrupt (the single result follows)");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "interrupted finish",
        is_finish,
    );
    let reasons = finished_stop_reasons(&env.events.lock().expect("events lock"));
    assert_eq!(reasons, vec!["interrupted".to_string()]);
}

/// Idle interrupt -> prompt -> interrupt -> ONE result -> the run ends exactly once;
/// Interrupt with nothing sent after it: the run still finishes, on the
/// aborted result, as interrupted.
#[test]
fn interrupt_without_replacement_finishes_as_interrupted() {
    let env = setup("interrupt-only");
    env.client
        .session_send(&env.session_id, "HOLD: slow tool call")
        .expect("held prompt");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "held echo",
        is_agent_message,
    );

    env.client
        .session_interrupt(&env.session_id)
        .expect("interrupt");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "interrupted finish",
        is_finish,
    );
    let reasons = finished_stop_reasons(&env.events.lock().expect("events lock"));
    assert_eq!(reasons, vec!["interrupted".to_string()]);
}

/// Positive control: no interrupt means the result completes the run exactly
/// as it always did.
#[test]
fn normal_result_completes_as_end_turn() {
    let env = setup("normal");
    env.client
        .session_send(&env.session_id, "PRIME")
        .expect("prompt");
    common::wait_for(&env.events, Duration::from_secs(15), "finish", is_finish);
    let reasons = finished_stop_reasons(&env.events.lock().expect("events lock"));
    assert_eq!(reasons, vec!["end_turn".to_string()]);
}

/// The bound: an interrupt whose aborted result never arrives must not arm a
/// suppression that swallows the next genuine result.
#[test]
fn abort_expectation_does_not_outlive_a_silent_interrupt() {
    let env = setup("silent-abort");
    env.client
        .session_send(&env.session_id, "HOLD-SILENT: slow tool call")
        .expect("held prompt");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "held echo",
        is_agent_message,
    );

    env.client
        .session_interrupt(&env.session_id)
        .expect("interrupt");
    env.client
        .session_send(&env.session_id, "Reply now")
        .expect("replacement prompt");
    common::wait_for(
        &env.events,
        Duration::from_secs(15),
        "genuine result",
        is_finish,
    );
    let reasons = finished_stop_reasons(&env.events.lock().expect("events lock"));
    assert_eq!(reasons, vec!["end_turn".to_string()]);
}
