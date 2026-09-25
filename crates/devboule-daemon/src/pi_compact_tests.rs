//! The compact-run tests: Paseo's one-compaction-at-a-time guard — its
//! refusal sentence, and the compaction frames that hold and release the
//! slot — and the bound that keeps a silent child from pinning a thread
//! (review A5-2 #3 and #4).

use super::super::{PiControl, PiReader};
use crate::session::event_pull::ConnHandle;
use crate::session::permission_broker::PermissionBroker;
use crate::session::{OutOfBandCommands, ReaderDispatch, SessionRuntime};
use devboule_protocol::SessionEvent;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A child that stays alive and absorbs whatever is written to its stdin
/// without ever answering: `ping -t` never reads the pipe (our frames sit
/// in its buffer) and never writes anything a response could be mistaken
/// for. A system binary, so the refusal test needs no `node` — whose
/// execution on this box is guarded, and has been flaky (see the report).
fn absorbing_child() -> (
    std::process::Child,
    Arc<Mutex<Option<std::process::ChildStdin>>>,
) {
    let mut child = std::process::Command::new("ping")
        .args(["-t", "127.0.0.1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("ping is a Windows system binary");
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    (child, stdin)
}

/// A fake Pi on raw pipes: the test's own reader delivers responses and
/// observes compaction frames, exactly as the session reader does.
fn fake_pi(
    script: &str,
) -> (
    std::process::Child,
    Arc<Mutex<Option<std::process::ChildStdin>>>,
    std::io::BufReader<std::process::ChildStdout>,
) {
    let mut child = std::process::Command::new("node")
        .args(["-e", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| {
            panic!(
                "node is not runnable here ({error}; kind={:?})",
                error.kind()
            )
        });
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    (child, stdin, stdout)
}

/// A runtime with a subscription, mirroring the other pi test modules.
fn attached_runtime(session_id: &str) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let runtime = SessionRuntime::for_acp(session_id.to_string(), None, Arc::clone(&broker));
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    (runtime, conn)
}

/// One pull: the system lines it carried (Paseo's compaction markers, as
/// `pi_view` shows them) come back first, the assistant lines second.
fn drain(conn: &ConnHandle) -> (Vec<String>, Vec<String>) {
    let (mut notices, mut messages) = (Vec::new(), Vec::new());
    for event in conn.pull_events() {
        match event.envelope.event {
            SessionEvent::SessionNotice { text, .. } => notices.push(text),
            SessionEvent::AgentMessage { text, .. } => messages.push(text),
            _ => {}
        }
    }
    (notices, messages)
}

/// Pull until the system line `text` appears; the assistant lines seen on
/// the way are handed back.
fn wait_for_notice(conn: &ConnHandle, text: &str) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (notices, messages) = drain(conn);
        if notices.iter().any(|notice| notice == text) {
            return messages;
        }
        assert!(
            Instant::now() < deadline,
            "no {text:?} system line yet: {notices:?} / {messages:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_second_compact_is_refused_while_the_first_is_outstanding() {
    // Paseo refuses a second `/compact` with its own sentence while one runs
    // (`pi/agent.ts:1821-1825`, thrown into the client's `[Error] …` line at
    // `agent-manager.ts:2381-2387`) and writes no second RPC — the refusal
    // returns before the write (review A5-2 #3). The child holds the first
    // compaction open by never answering, so the window is deterministic and
    // both runs settle synchronously: no timers, no node, no flake.
    let (mut child, stdin) = absorbing_child();
    let control = PiControl::new(stdin, Arc::new(AtomicU64::new(1)));
    let handler = super::PiOutOfBandCommands::new(Arc::new(control));
    let (runtime, conn) = attached_runtime("pi-compact-guard");

    handler.run_out_of_band("/compact fold it", &runtime);
    handler.run_out_of_band("/compact fold it again", &runtime);

    assert_eq!(
        drain(&conn).1,
        ["[Error] A Pi compact command is already running"],
        "Paseo's refusal sentence, shown as the assistant line it is — and, being \
         refused, nothing else: the refused compact writes no rpc, and the held \
         first one answers nothing"
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// A fake Pi whose compaction starts before its reply and ends 400 ms after
/// it — the window Paseo's guard lives in (`pi/agent.ts:1826` to `:2352`).
const FAKE_PI_ENDS_COMPACT_LATE: &str = r#"
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    if (frame.type === "compact") {
      process.stdout.write(JSON.stringify({ type: "compaction_start", reason: "manual" }) + "\n");
      process.stdout.write(JSON.stringify({ id: frame.id, type: "response", success: true, received: line }) + "\n");
      setTimeout(() => {
        process.stdout.write(JSON.stringify({ type: "compaction_end", reason: "manual" }) + "\n");
      }, 400);
      continue;
    }
    process.stdout.write(
      JSON.stringify({ id: frame.id, type: "response", success: true, received: line }) + "\n"
    );
  }
});
"#;

#[test]
fn the_guard_holds_until_the_compaction_ends_and_releases_after_it() {
    // Paseo's guard lifecycle end to end (`pi/agent.ts:1826,1856-1860,
    // 2344-2356`): an RPC that settled while its compaction is still
    // running keeps the slot — the second compact is refused — and pi's own
    // `compaction_end` releases it, so a third compact is accepted
    // (review A5-2 #3). One reader does both jobs exactly as production
    // does: it delivers the reply and observes the compaction frames on the
    // handler's own guard.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let (mut child, stdin, stdout) = fake_pi(FAKE_PI_ENDS_COMPACT_LATE);
    let reader_stdin = Arc::clone(&stdin);
    let control = Arc::new(PiControl::new(stdin, Arc::new(AtomicU64::new(1))));
    let handler = super::PiOutOfBandCommands::new(Arc::clone(&control));
    let (runtime, conn) = attached_runtime("pi-compact-lifecycle");
    let mut reader = PiReader::new(
        Vec::new(),
        SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: None,
        },
        PermissionBroker::for_test(Arc::new(|_, _| Ok(()))),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(AtomicU64::new(1)),
        Arc::clone(&control),
        reader_stdin,
        Arc::new(AtomicBool::new(true)),
    )
    .with_compact_guard(handler.compact_guard());
    let feeder_runtime = Arc::clone(&runtime);
    let feeder = std::thread::spawn(move || {
        let mut stdout = stdout;
        let mut line = String::new();
        loop {
            line.clear();
            match std::io::BufRead::read_line(&mut stdout, &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let _ = reader.feed(line.as_bytes(), &feeder_runtime);
        }
    });

    // The first compaction announces itself; its reply settles while the
    // compaction is still running, so the slot stays taken either way.
    handler.run_out_of_band("/compact one", &runtime);
    let messages = wait_for_notice(&conn, "Compacting...");
    assert!(
        messages.is_empty(),
        "a compaction in progress shows no assistant line yet: {messages:?}"
    );

    // Refused while it runs — Paseo's sentence, and nothing else.
    handler.run_out_of_band("/compact two", &runtime);
    assert_eq!(
        drain(&conn).1,
        ["[Error] A Pi compact command is already running"],
        "the guard still holds: the settled rpc did not release a compaction \
         that had already started"
    );

    // pi's own end marker releases the slot...
    wait_for_notice(&conn, "Context manually compacted");

    // ...so a third compact runs, and its start marker is the proof. No
    // pull may happen between the run and this loop: every pull drains what
    // it passes over, so the marker would be consumed by the very assertion
    // looking for it. The loop accumulates instead of discarding, and a
    // refusal would show up in the messages it reports.
    handler.run_out_of_band("/compact three", &runtime);
    let mut seen: Vec<String> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (notices, messages) = drain(&conn);
        seen.extend(notices);
        if seen.iter().any(|notice| notice.as_str() == "Compacting...") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the released slot never took a third compact: seen={seen:?} messages={messages:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = feeder.join();
}

#[test]
fn a_compact_whose_child_never_answers_ends_in_a_bounded_failure_and_frees_the_slot() {
    // review A5-2 #4: the compact wait is bounded — a live child that stops
    // answering must not pin a worker thread and its registration for the
    // session's life — and the round trip still ends in Paseo's failure
    // line, so the timeout is visible to the user rather than silent. The
    // bound is short here only so the test does not wait the production one.
    let (mut child, stdin) = absorbing_child();
    let control = PiControl::new(stdin, Arc::new(AtomicU64::new(1)));
    let handler = super::PiOutOfBandCommands::new(Arc::new(control))
        .with_compact_timeout(Duration::from_millis(150));
    let (runtime, conn) = attached_runtime("pi-compact-bound");

    handler.run_out_of_band("/compact silent child", &runtime);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (_, messages) = drain(&conn);
        if messages == ["[Error] Failed to compact context: Pi compact response timed out"] {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no bounded failure line: {messages:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // The slot is free again, so the next compact is accepted rather than
    // refused with Paseo's sentence.
    handler.run_out_of_band("/compact again", &runtime);
    let (_, messages) = drain(&conn);
    assert!(
        messages.is_empty(),
        "the second compact was accepted, not refused: {messages:?}"
    );

    let _ = child.kill();
    let _ = child.wait();
}
