//! Silence and liveness, moved whole out of `session_tests.rs` lines 1067-1262:
//! the threshold transition emitted once, the queued silence dropped when
//! output precedes a reattach or an exit lands first, the ACP roster notice on
//! leaving silent, the OS liveness probe that marks an exit without EOF, and an
//! elapsed time that keeps a recovered session's unknown life unknown. Every
//! line below is byte-identical to its text there apart from this header;
//! `drain` and `attach_tracked` are `pub(super)` in the provider this file
//! imports them from.

use super::tests::{attach_tracked, drain};
use super::*;

#[test]
fn silence_transition_is_emitted_once_after_the_threshold() {
    let runtime = Arc::new(SessionRuntime::new());
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);
    let _ = drain(&conn);
    let last_publish = runtime
        .stream
        .lock()
        .expect("stream lock")
        .last_publish
        .expect("new sessions have an observed start time");

    assert_eq!(
        runtime.mark_silent_if_due(
            last_publish + SESSION_SILENCE_THRESHOLD + Duration::from_millis(42)
        ),
        Some(SESSION_SILENCE_THRESHOLD.as_millis() as u64 + 42)
    );
    assert_eq!(
        drain(&conn),
        vec![SessionEvent::Silent {
            elapsed_ms: SESSION_SILENCE_THRESHOLD.as_millis() as u64 + 42,
        }]
    );
    assert_eq!(
        runtime
            .mark_silent_if_due(last_publish + SESSION_SILENCE_THRESHOLD + Duration::from_secs(1)),
        None
    );
    assert!(
        drain(&conn).is_empty(),
        "silence is a transition, not a tick"
    );
}

#[test]
fn queued_silence_is_dropped_when_output_precedes_a_reattach() {
    let runtime = Arc::new(SessionRuntime::new());
    let first = Arc::new(ConnHandle::new(1));
    attach_tracked(&runtime, &first);
    let _ = drain(&first);
    let last_publish = runtime
        .stream
        .lock()
        .expect("stream lock")
        .last_publish
        .expect("new sessions have an observed start time");

    runtime.mark_silent_if_due(last_publish + SESSION_SILENCE_THRESHOLD + Duration::from_millis(1));
    runtime.publish_output("resumed");
    runtime.detach_if_conn(first.id);

    let second = Arc::new(ConnHandle::new(2));
    attach_tracked(&runtime, &second);
    let events = drain(&second);
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, SessionEvent::Silent { .. })),
        "a reattached client must not receive stale silence: {events:?}"
    );
}

#[test]
fn silence_is_dropped_when_the_session_exits() {
    let runtime = Arc::new(SessionRuntime::new());
    let conn = Arc::new(ConnHandle::new(1));
    attach_tracked(&runtime, &conn);
    let _ = drain(&conn);
    let last_publish = runtime
        .stream
        .lock()
        .expect("stream lock")
        .last_publish
        .expect("new sessions have an observed start time");

    runtime.mark_silent_if_due(last_publish + SESSION_SILENCE_THRESHOLD + Duration::from_millis(1));
    runtime.finish(Some(7));

    assert_eq!(
        drain(&conn),
        vec![SessionEvent::Exit { code: Some(7) }],
        "exit must be the only terminal transition delivered after silence"
    );
}

#[test]
fn acp_publish_notifies_roster_when_leaving_silent() {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.transition_ready.store(true, Ordering::Release);
    let notified = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&notified);
    runtime.set_roster_notify(Arc::new(move || {
        flag.store(true, Ordering::SeqCst);
    }));
    let last_publish = runtime
        .stream
        .lock()
        .expect("stream lock")
        .last_publish
        .expect("new sessions have an observed start time");
    runtime.mark_silent_if_due(last_publish + SESSION_SILENCE_THRESHOLD + Duration::from_millis(1));
    assert!(
        matches!(
            runtime.lock_stream().expect("stream").disposition,
            Disposition::Silent
        ),
        "precondition: session is Silent"
    );
    runtime.publish_agent_event(
        SessionEvent::AgentMessage {
            message_id: Some("m1".to_string()),
            text: "back".to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
        },
        None,
    );
    assert!(
        matches!(
            runtime.lock_stream().expect("stream").disposition,
            Disposition::Running
        ),
        "ACP output must return the stream to Running"
    );
    assert!(
        notified.load(Ordering::SeqCst),
        "ACP Silent→Live must notify the sessions_watch roster, like PTY output"
    );
}

#[cfg(windows)]
fn spawn_innocuous_os_child() -> std::process::Child {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new("cmd.exe")
        .args(["/d", "/c", "ping", "-n", "30", "127.0.0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("spawn innocuous ping")
}

#[cfg(windows)]
#[test]
fn os_liveness_observation_marks_exited_without_eof() {
    use std::os::windows::io::AsRawHandle;
    let runtime = Arc::new(SessionRuntime::new());
    runtime.transition_ready.store(true, Ordering::Release);
    let mut child = spawn_innocuous_os_child();
    let handle = ProcessHandle::duplicate(AsRawHandle::as_raw_handle(&child)).expect("duplicate");
    runtime.install_os_handle(handle);
    assert!(!runtime.process_exited(), "a live OS process is not Exited");
    assert!(
        !runtime.observe_os_liveness(),
        "an alive process must not be marked exited"
    );
    child.kill().expect("kill ping");
    let _ = child.wait();
    assert!(
        runtime.observe_os_liveness(),
        "OS observation must mark Exited without waiting on the PTY/ACP pipe EOF"
    );
    assert!(runtime.process_exited());
    let stream = runtime.lock_stream().expect("stream");
    assert!(
        matches!(stream.disposition, Disposition::Exited { .. }),
        "disposition must be Exited from the OS query, not from child.wait: {:?}",
        stream.disposition
    );
}

#[test]
fn elapsed_time_uses_exit_for_ended_and_stays_unknown_for_recovered() {
    let now = Instant::now();
    let last_publish = Some(now - Duration::from_secs(3600));
    let exit_at = Some(now - Duration::from_secs(7));

    assert_eq!(
        elapsed_ms_since_last_life(last_publish, exit_at, true, now),
        Some(7_000)
    );
    assert_eq!(
        elapsed_ms_since_last_life(last_publish, None, false, now),
        Some(3_600_000)
    );
    assert_eq!(
        elapsed_ms_since_last_life(None, None, true, now),
        None,
        "journal-only recovered sessions have no monotonic timestamp"
    );
}
