//! The pi command-list tests: the `get_commands` reply that becomes the
//! menu, the interleaving it has to survive, and the seeds the list keeps
//! when the reply never comes.

use super::super::{PiControl, PiReader};
use super::{await_commands_reply, begin_get_commands, PiCommandsReply};
use crate::session::event_pull::ConnHandle;
use crate::session::permission_broker::PermissionBroker;
use crate::session::{write_child_stdin, ReaderDispatch, SessionRuntime};
use devboule_protocol::{AvailableCommandView, SessionEvent};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// A fake Pi whose `get_commands` answer waits for the user's prompt: two
/// `extension_ui_request` frames interleave first, and the reply itself is
/// owed until a prompt has been read. A prompt that waited on that reply
/// would deadlock the stub, which the test bounds rather than hangs on.
const PI_ANSWERS_THE_LIST_ON_A_PROMPT: &str = r#"
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
    if (frame.type === "prompt") {
      setTimeout(reply, 50);
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

#[test]
fn the_list_reply_lands_after_interleaved_ui_frames_and_a_prompt_that_did_not_wait() {
    // The brief's first case, measured against the stub: the reply arrives
    // after two `extension_ui_request` frames AND after a user prompt was
    // sent, and the prompt must not have waited for it. The stub answers the
    // list only once it has read a prompt, so a send path that blocked on
    // the reply would deadlock — bounded by `recv_timeout` so that failure
    // is a red rather than a hang.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let (mut child, stdin, stdout) = fake_pi(PI_ANSWERS_THE_LIST_ON_A_PROMPT);
    let control = Arc::new(PiControl::new(
        Arc::clone(&stdin),
        Arc::new(AtomicU64::new(1)),
    ));
    let reply = begin_get_commands(&control);
    let (runtime, conn) = attached_runtime("pi-commands-flow");
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

    let prompt_stdin = Arc::clone(&stdin);
    let (sent_tx, sent_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let frame = serde_json::json!({
            "id": "p-90",
            "type": "prompt",
            "message": "/goal keep it short",
        })
        .to_string();
        let _ = write_child_stdin(&prompt_stdin, format!("{frame}\n").as_bytes(), "Pi");
        let _ = sent_tx.send(());
    });
    sent_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the prompt waited for the get_commands reply; a prompt must not block on it");

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
