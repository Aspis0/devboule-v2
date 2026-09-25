//! The out-of-band command tests: `/compact` and `/autocompact` leave as
//! pi's own RPCs before any turn starts, every other slash text stays a
//! prompt, and the argument parsing is Paseo's rule for rule.

use super::super::PiControl;
use super::{
    parse_auto_compact_mode, parse_slash_invocation, AutoCompactMode, PiOutOfBandCommands,
};
use crate::session::tests::{
    attach_live_agent_for_test, insert_live_agent_with_out_of_band, test_owner,
    tmp_delete_registry, RecordingWriter,
};
use crate::session::{OutOfBandCommands, SessionRuntime};
use devboule_protocol::SessionKind;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A fake Pi that answers every control frame with a response naming the
/// frame's own id and echoes the raw line it read, so a test can assert on
/// the bytes pi was sent.
const FAKE_PI_ANSWERS: &str = r#"
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    process.stdout.write(
      JSON.stringify({
        id: frame.id,
        type: "response",
        success: true,
        received: line,
      }) + "\n"
    );
  }
});
"#;

struct AnsweringPi {
    child: std::process::Child,
    control: Arc<PiControl>,
    answers: Arc<Mutex<Vec<serde_json::Value>>>,
    reader: std::thread::JoinHandle<()>,
}

/// The fake Pi plus the reader thread that correlates its answers by id —
/// the same shape the session's reader gives a live child.
fn answering_pi() -> AnsweringPi {
    let mut child = std::process::Command::new("node")
        .args(["-e", FAKE_PI_ANSWERS])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| {
            panic!(
                "node is not runnable here ({error}; kind={:?})",
                error.kind()
            )
        });
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let mut stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let control = Arc::new(PiControl::new(
        Arc::clone(&stdin),
        Arc::new(AtomicU64::new(1)),
    ));
    let answers = Arc::new(Mutex::new(Vec::new()));
    let reader_control = Arc::clone(&control);
    let reader_answers = Arc::clone(&answers);
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut line = String::new();
        loop {
            line.clear();
            match stdout.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let Ok(answer) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if let Ok(mut recorded) = reader_answers.lock() {
                recorded.push(answer.clone());
            }
            let _ = reader_control.deliver(&answer);
        }
    });
    AnsweringPi {
        child,
        control,
        answers,
        reader,
    }
}

impl AnsweringPi {
    /// Stop the fake and hand back every raw line it was sent, in order.
    fn sent_lines(mut self) -> Vec<String> {
        let lines = self
            .answers
            .lock()
            .expect("answers")
            .iter()
            .filter_map(|answer| answer["received"].as_str().map(str::to_string))
            .collect();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.reader.join();
        lines
    }
}

fn wait_for_frames(answers: &Arc<Mutex<Vec<serde_json::Value>>>, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if answers.lock().expect("answers").len() >= count {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "pi received fewer than {count} frames: {:?}",
            answers.lock().expect("answers")
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One pull: the user messages it carried come back, the assistant messages
/// are kept aside. Both readings share a pull because a pull drains what it
/// carries — and the reply to an rpc can land before the assertion that
/// would otherwise have read the outcome.
fn drain(conn: &crate::session::event_pull::ConnHandle, outcomes: &mut Vec<String>) -> Vec<String> {
    let mut recorded = Vec::new();
    for event in conn.pull_events() {
        match event.envelope.event {
            devboule_protocol::SessionEvent::AgentUserMessage { text, .. } => recorded.push(text),
            devboule_protocol::SessionEvent::AgentMessage { text, .. } => outcomes.push(text),
            _ => {}
        }
    }
    recorded
}

#[test]
fn compact_and_autocompact_reach_pi_as_rpc_frames_and_goal_as_prompt_text() {
    // The brief's third case, driven through the send path: the two native
    // commands go out as pi's own control frames and never as prompts, the
    // input is still recorded, and no turn starts for them — while `/goal x`
    // keeps the ordinary road and lands in the plain-text writer.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-pi-oob", "process-pi-oob");
    let session_id = "pi-out-of-band";
    let pi = answering_pi();
    let hook = Arc::new(PiOutOfBandCommands {
        control: Arc::clone(&pi.control),
    });
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_out_of_band(
        &registry,
        session_id,
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::clone(&received))),
        Some(hook),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 91);

    registry
        .send_with_subscription(session_id, 91, "/compact fold it", &[], &[], &owner, &conn)
        .expect("compact send");
    registry
        .send_with_subscription(session_id, 91, "/autocompact on", &[], &[], &owner, &conn)
        .expect("autocompact send");

    // Neither command began a turn: nothing was written as a prompt, so the
    // runtime never moved into one.
    assert!(
        !runtime.is_turn_active(runtime.turn_counter()),
        "an out-of-band command runs without allocating a turn"
    );
    assert!(
        received.lock().expect("writer").is_empty(),
        "the two commands reached pi as frames, not through the prompt writer"
    );

    // Both RPCs reached the child (the fake echoes each one).
    wait_for_frames(&pi.answers, 2);

    // The input is still what the person typed, recorded like any accepted
    // input (Paseo records the submitted prompt too), and every outcome the
    // rpcs already published is kept by the same pull.
    let mut outcomes: Vec<String> = Vec::new();
    let recorded = drain(&conn, &mut outcomes);
    assert_eq!(recorded, ["/compact fold it", "/autocompact on"]);

    // /goal x is an ordinary slash text: it goes to the writer untouched.
    registry
        .send_with_subscription(session_id, 91, "/goal x", &[], &[], &owner, &conn)
        .expect("goal send");
    let written = String::from_utf8(received.lock().expect("writer").clone()).expect("utf8");
    assert!(
        written.ends_with("/goal x"),
        "any other slash text reaches the provider as prompt text, got {written:?}"
    );
    assert!(
        runtime.is_turn_active(runtime.turn_counter()),
        "the ordinary prompt road still begins its turn, which the commands must not"
    );

    // The autocompact outcome is shown the way Paseo shows it: the same
    // sentence, once the RPC has answered. A successful `/compact` adds
    // nothing — Paseo's handler emits nothing after its RPC either.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        drain(&conn, &mut outcomes);
        if outcomes == ["Auto-compaction enabled."] {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no outcome message yet: {outcomes:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // Last, once every frame the child was sent has been echoed: the bytes
    // pi received.
    let lines = pi.sent_lines();
    let frames = lines
        .iter()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("frame"))
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 2, "exactly the two RPCs: {frames:?}");
    assert_eq!(frames[0]["type"], "compact");
    assert_eq!(frames[0]["customInstructions"], "fold it");
    assert_eq!(frames[1]["type"], "set_auto_compaction");
    assert_eq!(frames[1]["enabled"], true);
    assert!(
        frames.iter().all(|frame| frame["type"] != "prompt"),
        "neither command was sent as a prompt: {frames:?}"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unusable_autocompact_argument_is_paseos_usage_line_and_writes_nothing() {
    // A refusal the handler can answer without touching pi: the message is
    // Paseo's own sentence, and no frame is written for it.
    let stdin: Arc<Mutex<Option<std::process::ChildStdin>>> = Arc::new(Mutex::new(None));
    let control = Arc::new(PiControl::new(
        Arc::clone(&stdin),
        Arc::new(AtomicU64::new(1)),
    ));
    let hook = PiOutOfBandCommands {
        control: Arc::clone(&control),
    };
    assert!(
        hook.handles_out_of_band("/autocompact maybe"),
        "the command is recognised; its argument is what is refused"
    );
    let broker =
        crate::session::permission_broker::PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let runtime = SessionRuntime::for_acp("pi-oob-usage".to_string(), None, Arc::clone(&broker));
    let conn = crate::session::event_pull::ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "pi-oob-usage",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );

    hook.run_out_of_band("/autocompact maybe", &runtime);

    let messages = conn
        .pull_events()
        .into_iter()
        .filter_map(|event| match event.envelope.event {
            devboule_protocol::SessionEvent::AgentMessage { text, .. } => Some(text),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        messages,
        ["[Error] Usage: /autocompact [on|off|toggle]"],
        "Paseo's usage line, published as the assistant line it is"
    );
    assert!(
        control.pending.lock().expect("pending").is_empty(),
        "a refused argument never reaches pi"
    );
}

#[test]
fn a_slash_text_is_a_command_only_for_the_two_pi_handles() {
    // Every other slash text must fall through to the prompt road, so the
    // recognition is exactly Paseo's `tryHandleOutOfBand` dispatch: compact
    // and autocompact (any case), nothing else.
    let stdin: Arc<Mutex<Option<std::process::ChildStdin>>> = Arc::new(Mutex::new(None));
    let hook = PiOutOfBandCommands {
        control: Arc::new(PiControl::new(stdin, Arc::new(AtomicU64::new(1)))),
    };
    assert!(hook.handles_out_of_band("/compact"));
    assert!(hook.handles_out_of_band("/COMPACT fold everything"));
    assert!(hook.handles_out_of_band("/autocompact"));
    assert!(hook.handles_out_of_band("/autocompact off"));
    assert!(!hook.handles_out_of_band("/goal x"));
    assert!(!hook.handles_out_of_band("/skill:pdf go"));
    assert!(!hook.handles_out_of_band("compact"));
    assert!(!hook.handles_out_of_band("/a/b"));
}

#[test]
fn the_slash_parse_is_paseos_rule_for_rule() {
    // Paseo `pi/agent.ts:1797-1811`: trim, drop a lone `/`, take the name up
    // to the first whitespace, refuse a name containing a slash, keep the
    // trimmed remainder as args only when it is non-empty.
    let invocation = parse_slash_invocation("  /compact   fold it  ").expect("a command");
    assert_eq!(invocation.name, "compact");
    assert_eq!(invocation.args.as_deref(), Some("fold it"));

    let bare = parse_slash_invocation("/autocompact").expect("a command");
    assert_eq!(bare.name, "autocompact");
    assert!(
        bare.args.is_none(),
        "no remainder means no args, not empty ones"
    );

    assert!(
        parse_slash_invocation("/").is_none(),
        "a lone slash is not a command"
    );
    assert!(
        parse_slash_invocation("/a/b").is_none(),
        "a name with a slash is not a command"
    );
    assert!(
        parse_slash_invocation("compact").is_none(),
        "no slash, no command"
    );
    assert!(parse_slash_invocation("/").is_none());
    assert!(parse_slash_invocation("").is_none());
}

#[test]
fn autocompact_resolves_its_argument_the_way_paseo_does() {
    // Paseo `pi/agent.ts:362-375`: absent means toggle, the four affirmative
    // and the four negative spellings, and anything else is unknown — which
    // the handler answers with the usage line rather than an RPC.
    for spelling in [None, Some("toggle")] {
        assert_eq!(
            parse_auto_compact_mode(spelling),
            AutoCompactMode::Toggle,
            "{spelling:?} toggles"
        );
    }
    for spelling in ["on", "true", "enable", "enabled", " ON ", "Enabled"] {
        assert_eq!(
            parse_auto_compact_mode(Some(spelling)),
            AutoCompactMode::Enabled,
            "{spelling} enables"
        );
    }
    for spelling in ["off", "false", "disable", "disabled", " OFF "] {
        assert_eq!(
            parse_auto_compact_mode(Some(spelling)),
            AutoCompactMode::Disabled,
            "{spelling} disables"
        );
    }
    assert_eq!(
        parse_auto_compact_mode(Some("maybe")),
        AutoCompactMode::Unknown
    );
}
