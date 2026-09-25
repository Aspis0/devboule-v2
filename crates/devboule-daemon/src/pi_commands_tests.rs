//! The pi command-list tests: the `get_commands` reply that becomes the
//! menu, the interleaving it has to survive, and the seeds the list keeps
//! when the reply never comes.

use super::super::{PiControl, PiReader};
use super::{await_commands_reply, begin_get_commands, refusal_log_line, PiCommandsReply};
use crate::session::event_pull::ConnHandle;
use crate::session::permission_broker::PermissionBroker;
use crate::session::tests::{
    attach_live_agent_for_test, insert_live_agent_with_kind_writer_and_sink, test_owner,
    tmp_delete_registry,
};
use crate::session::{write_child_stdin, ReaderDispatch, SessionRuntime};
use devboule_protocol::{AvailableCommandView, SessionEvent, SessionKind};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// A fake Pi whose `get_commands` answer waits for a release the test sends
/// directly: two `extension_ui_request` frames interleave first, and the
/// reply itself is owed until a `test_release` frame arrives — which the test
/// writes only after the prompt send has returned. A send path that waited
/// on the list, before or after writing the prompt, would deadlock here:
/// the reply cannot exist before the send returns. Bounded by `recv_timeout`
/// so the failure is a red rather than a hang.
const PI_HOLDS_THE_LIST_UNTIL_RELEASED: &str = r#"
let buffered = "";
let commandsId = null;
const reply = () => process.stdout.write(JSON.stringify({
  id: commandsId,
  type: "response",
  command: "get_commands",
  success: true,
  data: { commands: [
    { name: "goal", description: "Set the session goal", source: "extension", input: { hint: "<objective>" } },
    { name: "skill:pdf", source: "skill" }
  ] }
}) + "\n");
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    if (frame.type === "get_commands") {
      commandsId = frame.id;
      process.stdout.write(JSON.stringify({ type: "extension_ui_request", method: "notify", message: "stub-ui-one" }) + "\n");
      process.stdout.write(JSON.stringify({ type: "extension_ui_request", method: "notify", message: "stub-ui-two" }) + "\n");
      continue;
    }
    if (frame.type === "test_release") {
      reply();
      continue;
    }
  }
});
"#;

/// A fake Pi that refuses `get_commands` the way a build without it would,
/// and answers a prompt with a recorded `turn_end` so the test can prove the
/// session still turns after the refusal.
const PI_REFUSES_THE_LIST: &str = r#"
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    if (frame.type === "get_commands") {
      process.stdout.write(JSON.stringify({
        id: frame.id,
        type: "response",
        command: "get_commands",
        success: false,
        error: "Unknown command: get_commands"
      }) + "\n");
      continue;
    }
    if (frame.type === "prompt") {
      process.stdout.write(JSON.stringify({
        type: "turn_end",
        message: { role: "assistant", content: [{ type: "text", text: "OK" }],
          model: "test-model", usage: { inputTokens: 1, totalTokens: 1 },
          stopReason: "stop" }
      }) + "\n");
    }
  }
});
"#;

fn fake_pi(
    script: &str,
) -> (
    std::process::Child,
    Arc<Mutex<Option<std::process::ChildStdin>>>,
    std::io::BufReader<std::process::ChildStdout>,
) {
    let mut child = std::process::Command::new("node")
        .args(["-e", script])
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
    let stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    (child, stdin, stdout)
}

/// A runtime with a subscription, mirroring the ACP/Pi helpers: published
/// events can be pulled from `conn`.
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

fn reader_for(
    control: Arc<PiControl>,
    stdin: &Arc<Mutex<Option<std::process::ChildStdin>>>,
) -> PiReader {
    PiReader::new(
        Vec::new(),
        SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: None,
        },
        PermissionBroker::for_test(Arc::new(|_, _| Ok(()))),
        Arc::new(Mutex::new(std::collections::HashMap::new())),
        Arc::new(AtomicU64::new(1)),
        control,
        Arc::clone(stdin),
        Arc::new(AtomicBool::new(true)),
    )
}

/// Pull until one `AvailableCommands` event arrives, or fail the deadline.
fn pull_commands(conn: &ConnHandle, within: Duration) -> Vec<AvailableCommandView> {
    let deadline = Instant::now() + within;
    loop {
        for pending in conn.pull_events() {
            if let SessionEvent::AvailableCommands { commands } = pending.envelope.event {
                return commands;
            }
        }
        assert!(
            Instant::now() < deadline,
            "no AvailableCommands event within {within:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn names(commands: &[AvailableCommandView]) -> Vec<&str> {
    commands
        .iter()
        .map(|command| command.name.as_str())
        .collect()
}

/// One staged timeout decision: the registration, the channel behind it, and
/// what the waiter must answer.
fn staged_timeout(
    registered: bool,
    send: Option<Result<serde_json::Value, String>>,
) -> Option<String> {
    let stdin: Arc<Mutex<Option<std::process::ChildStdin>>> = Arc::new(Mutex::new(None));
    let control = Arc::new(PiControl::new(
        Arc::clone(&stdin),
        Arc::new(AtomicU64::new(1)),
    ));
    let (sender, held) = mpsc::channel();
    if registered {
        control
            .pending
            .lock()
            .expect("pending")
            .insert("c-1".to_string(), sender.clone());
    }
    if let Some(answer) = send {
        sender.send(answer).expect("the staged answer lands");
    }
    let reply = PiCommandsReply {
        control: Arc::clone(&control),
        id: Some("c-1".to_string()),
        response: held,
    };
    super::on_commands_timeout(&reply, Duration::from_secs(60))
}

#[test]
fn the_timeout_decision_gives_each_race_state_its_own_answer() {
    // Staged: the waiter's own removal wins, the reader's claim wins for a
    // buffered success, the channel's end and a buffered failure speak, and
    // an empty channel with no entry belongs to the waiter — removal and
    // send share one lock, so nothing is still in flight.
    let success = || {
        serde_json::from_str::<serde_json::Value>(
            r#"{"id":"c-1","type":"response","command":"get_commands","success":true}"#,
        )
        .expect("recorded reply")
    };
    let failure = || {
        serde_json::from_str::<serde_json::Value>(
            r#"{"id":"c-1","type":"response","command":"get_commands","success":false,"error":"no"}"#,
        )
        .expect("recorded reply")
    };
    assert!(
        staged_timeout(true, None)
            .expect("the waiter won")
            .contains("within 60s"),
        "an unanswered timeout still leaves the seeds"
    );
    assert_eq!(
        staged_timeout(false, Some(Ok(success()))),
        None,
        "a claimed success already published its list"
    );
    assert!(
        staged_timeout(false, Some(Ok(failure())))
            .expect("a claimed failure still needs the seeds")
            .contains("refused"),
        "a claimed failure leaves the seeds, not silence"
    );
    assert!(
        staged_timeout(false, Some(Err("gone".to_string())))
            .expect("the channel's end still needs the seeds")
            .contains("gone"),
        "a drained registration leaves the seeds with the end's reason"
    );
    assert_eq!(
        staged_timeout(false, None),
        Some("pi get_commands got no reply within 60s".to_string()),
        "entry gone and channel empty: with removal and send under one lock, \
         no answer is still in flight, so nobody owns this but the waiter"
    );
}

#[test]
fn the_list_reply_lands_after_interleaved_ui_frames_and_a_prompt_that_did_not_wait() {
    // The brief's first case, measured against the stub through the real
    // send path: the reply arrives after two
    // `extension_ui_request` frames, and only after the test releases it —
    // which happens after `send_with_subscription` has returned. A send path
    // blocked on the command list, before or after writing the prompt, could
    // never return to let the release through, so the 5 s bound is the red.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let (mut child, stdin, stdout) = fake_pi(PI_HOLDS_THE_LIST_UNTIL_RELEASED);
    let control = Arc::new(PiControl::new(
        Arc::clone(&stdin),
        Arc::new(AtomicU64::new(1)),
    ));
    let reply = begin_get_commands(&control);
    // The session a composer sends through: its writer is Pi's own, so the
    // production send path writes the prompt frame this stub waits for.
    let (dir, registry, journal) = tmp_delete_registry();
    let registry = Arc::new(registry);
    let owner = test_owner("S-1-5-21-pi-list", "process-pi-list");
    let session_id = "pi-list-flow";
    let writer = super::super::PiWriter {
        stdin: Arc::clone(&stdin),
        next_id: Arc::new(AtomicU64::new(100)),
        pending: Vec::new(),
    };
    let runtime = insert_live_agent_with_kind_writer_and_sink(
        &registry,
        session_id,
        owner.clone(),
        SessionKind::Pi,
        Box::new(writer),
        None,
        None,
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 97);
    let mut reader = reader_for(Arc::clone(&control), &stdin).with_commands_reply(reply);
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

    let send_registry = Arc::clone(&registry);
    let send_owner = owner.clone();
    let send_conn = Arc::clone(&conn);
    let (sent_tx, sent_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = send_registry.send_with_subscription(
            session_id,
            97,
            "/goal keep it short",
            &[],
            &[],
            &send_owner,
            &send_conn,
        );
        let _ = sent_tx.send(result);
    });
    sent_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the send path waited for the get_commands reply; a prompt must not block on it")
        .expect("the send lands");
    // The send returned with the reply still owed: release it now, straight
    // past the production send path, and the list that follows proves the
    // send never waited for it.
    write_child_stdin(&stdin, b"{\"type\":\"test_release\"}\n", "Pi")
        .expect("the release is written");

    let commands = pull_commands(&conn, Duration::from_secs(10));
    assert_eq!(
        names(&commands),
        ["compact", "autocompact", "goal", "skill:pdf"],
        "the two seeds first, then the reply's own entries, despite the interleaving"
    );
    assert_eq!(commands[0].hint.as_deref(), Some("[instructions]"));
    assert_eq!(commands[1].hint.as_deref(), Some("[on|off|toggle]"));
    assert_eq!(
        commands[2].hint.as_deref(),
        Some("<objective>"),
        "pi's own input.hint is kept, which Paseo drops"
    );
    assert_eq!(commands[3].description, "skill", "source as description");

    let _ = child.kill();
    let _ = child.wait();
    let _ = feeder.join();
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_list_refusal_log_never_carries_pis_error_text() {
    // review A5-2 #9: pi's `error` field is untrusted — a provider or an
    // extension can put a local path or a config value in it — so the one
    // daemon log line is a fixed sentence plus the text's length.
    let payload = r"C:\Users\dev\secret-config.toml";
    let line = refusal_log_line(payload);
    assert!(
        !line.contains("secret-config.toml"),
        "pi's error text must not reach the log: {line}"
    );
    assert_eq!(
        line,
        format!(
            "pi get_commands was refused (pi's error text was {} characters long)",
            payload.len()
        )
    );
    // The line names characters, not UTF-8 bytes: five CJK-and-dash glyphs
    // are thirteen bytes and must still read five.
    let wide = "\u{6a21}\u{578b}-\u{8a2d}\u{5b9a}";
    assert_eq!(wide.chars().count(), 5);
    let wide_line = refusal_log_line(wide);
    assert!(
        !wide_line.contains(wide),
        "pi's error text must not reach the log: {wide_line}"
    );
    assert_eq!(
        wide_line,
        "pi get_commands was refused (pi's error text was 5 characters long)"
    );
}

#[test]
fn a_failed_reply_publishes_only_the_seeds_and_the_session_still_turns() {
    // The brief's second case, half one: `success: false` leaves the list at
    // the seeds (the waiter asks, logs once, and publishes what pi's reply
    // does not carry), and nothing about the session stops working.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let (mut child, stdin, stdout) = fake_pi(PI_REFUSES_THE_LIST);
    let control = Arc::new(PiControl::new(
        Arc::clone(&stdin),
        Arc::new(AtomicU64::new(1)),
    ));
    let reply = begin_get_commands(&control);
    let (runtime, conn) = attached_runtime("pi-commands-refused");
    let mut reader = reader_for(Arc::clone(&control), &stdin).with_commands_reply(reply);
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

    let commands = pull_commands(&conn, Duration::from_secs(10));
    assert_eq!(
        names(&commands),
        ["compact", "autocompact"],
        "a refused get_commands leaves only the seeds"
    );
    assert_eq!(commands[0].hint.as_deref(), Some("[instructions]"));
    assert_eq!(commands[1].hint.as_deref(), Some("[on|off|toggle]"));

    // The session works: a prompt still reaches the child and its turn end
    // still lands as the finish the transcript hangs on.
    let frame = serde_json::json!({
        "id": "p-91",
        "type": "prompt",
        "message": "hello",
    })
    .to_string();
    write_child_stdin(&stdin, format!("{frame}\n").as_bytes(), "Pi")
        .expect("the prompt is written");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if conn
            .pull_events()
            .into_iter()
            .any(|pending| matches!(pending.envelope.event, SessionEvent::AgentFinished { .. }))
        {
            break;
        }
        assert!(Instant::now() < deadline, "no turn end after the refusal");
        std::thread::sleep(Duration::from_millis(20));
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = feeder.join();
}

#[test]
fn a_reply_that_never_comes_leaves_only_the_seeds() {
    // The brief's second case, half two: with no answer at all — here, a
    // timeout against a registered waiter nothing ever answers — the list is
    // the seeds, the waiter's registration is cleaned up, and the session
    // keeps publishing what comes after.
    let stdin: Arc<Mutex<Option<std::process::ChildStdin>>> = Arc::new(Mutex::new(None));
    let control = Arc::new(PiControl::new(
        Arc::clone(&stdin),
        Arc::new(AtomicU64::new(1)),
    ));
    // Registered exactly as `begin` registers one, without a child to write
    // to: what this pins is the wait's answer.
    let (sender, held) = mpsc::channel();
    control
        .pending
        .lock()
        .expect("pending")
        .insert("c-1".to_string(), sender);
    let reply = PiCommandsReply {
        control: Arc::clone(&control),
        id: Some("c-1".to_string()),
        response: held,
    };
    let (runtime, conn) = attached_runtime("pi-commands-silence");
    await_commands_reply(reply, &runtime, Duration::from_millis(200));

    let commands = pull_commands(&conn, Duration::from_secs(5));
    assert_eq!(
        names(&commands),
        ["compact", "autocompact"],
        "no reply leaves only the seeds"
    );
    assert!(
        control.pending.lock().expect("pending").is_empty(),
        "the timed-out waiter's registration is not left behind"
    );

    // The session still works: a later row still derives its events.
    let mut reader = reader_for(Arc::clone(&control), &stdin);
    let turn_end = serde_json::from_str::<serde_json::Value>(
        r#"{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"OK"}],"model":"test-model","usage":{"input":1,"totalTokens":1},"stopReason":"stop"}}"#,
    )
    .expect("recorded turn_end");
    reader
        .dispatch_value(turn_end, &runtime)
        .expect("the later row dispatches");
    assert!(
        conn.pull_events()
            .into_iter()
            .any(|pending| matches!(pending.envelope.event, SessionEvent::AgentFinished { .. })),
        "the silence broke nothing downstream"
    );
}
