//! Shared scaffolding for the Codex command tests: a temp Codex home and
//! workspace, and a fake `codex app-server` child that records the requests it
//! is given. No real CLI is ever asked to run a model.
//!
//! Declared as a test-only module of `codex_client` so the two command test
//! files — the requests on the wire, and the events the reader publishes — draw
//! on one harness instead of each keeping a copy.

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use super::super::event_pull::ConnHandle;
use super::super::{OutOfBandCommands, SessionRuntime};
use super::{CodexCommands, CodexOutOfBand, CodexState, CodexWriter};
use crate::codex_view::catalog_from_response;

/// The stdin handle the writer and the steerer share inside one test. Only the
/// registry holds the child's real stdin, so a test that asks what two of them
/// wrote has to hand them the same one.
pub(super) type SharedStdin = Arc<Mutex<Option<std::process::ChildStdin>>>;

/// A fake `codex app-server` (node): answers the handshake, records every
/// request on `FAKE_CODEX_COMMANDS` as `method<TAB>params`, records its own
/// argv on `FAKE_CODEX_ARGV`, and answers any request the handshake does not
/// cover with the JSON in `FAKE_CODEX_ANSWER` (`{}` by default — an app-server
/// result object, so a `thread/goal/set` answers clean and a canned
/// `{"error": …}` answers refused).
pub(super) const FAKE_CODEX: &str = r#"
const fs = require("fs");
const commands = process.env.FAKE_CODEX_COMMANDS || "";
const argvFile = process.env.FAKE_CODEX_ARGV || "";
const answer = process.env.FAKE_CODEX_ANSWER || "{}";
if (argvFile) fs.writeFileSync(argvFile, process.argv.slice(1).join("\n"));
let buf = "";
process.stdin.on("data", (chunk) => {
  buf += chunk.toString();
  let nl;
  while ((nl = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, nl);
    buf = buf.slice(nl + 1);
    if (!line.trim()) continue;
    let msg;
    try { msg = JSON.parse(line); } catch { continue; }
    if (msg.id === undefined || msg.id === null) continue;
    let result = {};
    if (msg.method === "initialize") result = { userAgent: "fake-codex" };
    else if (msg.method === "model/list") result = { data: [{ id: "fake-model", isDefault: true }] };
    else if (msg.method === "thread/start") result = { thread: { id: "thread-fake" } };
    else result = JSON.parse(answer);
    if (commands) fs.appendFileSync(commands, msg.method + "\t" + JSON.stringify(msg.params || {}) + "\n");
    process.stdout.write(JSON.stringify({ id: msg.id, result }) + "\n");
  }
});
"#;

/// A temp Codex home, a temp workspace, and the record files the fake child
/// writes. Removed when the test ends.
pub(super) struct Fixture {
    dir: PathBuf,
    commands_file: PathBuf,
    argv_file: PathBuf,
}

impl Fixture {
    pub(super) fn new(tag: &str) -> Self {
        let dir = crate::test_dirs::test_temp_dir(&format!("devboule-codex-{tag}"));
        let fixture = Self {
            commands_file: dir.join("commands.tsv"),
            argv_file: dir.join("argv.txt"),
            dir,
        };
        // Both halves of the surface exist before anything reads them: the
        // spawn path refuses a child whose cwd is missing, and an empty
        // `prompts` directory has to be absent rather than unreadable.
        std::fs::create_dir_all(fixture.home()).expect("the temp Codex home");
        std::fs::create_dir_all(fixture.cwd()).expect("the temp workspace");
        fixture
    }

    pub(super) fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    pub(super) fn cwd(&self) -> PathBuf {
        self.dir.join("cwd")
    }

    /// Where the fake child writes the argv it was started with.
    pub(super) fn argv_file(&self) -> PathBuf {
        self.argv_file.clone()
    }

    pub(super) fn write(&self, relative: &str, content: &str) {
        let path = self
            .dir
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the parent of a fixture file");
        }
        std::fs::write(path, content).expect("a fixture file is written");
    }

    /// The command surface of a session starting in this fixture. `picked`
    /// puts one custom prompt in the home and one skill in the workspace;
    /// without it the surface is the built-ins only.
    pub(super) fn commands(&self, picked: bool, goals_enabled: bool) -> Arc<CodexCommands> {
        if picked {
            self.write(
                "home/prompts/commit.md",
                "---\ndescription: Draft it\n---\nOn $1: $ARGUMENTS\n",
            );
            self.write(
                "cwd/.codex/skills/plotting/SKILL.md",
                "---\nname: plotting\ndescription: Draw it\n---\nSteps.\n",
            );
        }
        Arc::new(CodexCommands::new(
            &self.home(),
            Some(&self.cwd()),
            goals_enabled,
        ))
    }

    /// Spawn the fake child. The caller takes stdin from it and reads its
    /// answers, so no half of the pipe is ever held twice.
    pub(super) fn child(&self, answer: Option<&str>) -> std::process::Child {
        let mut command = std::process::Command::new("node");
        command
            .args(["-e", FAKE_CODEX, "--"])
            .env("FAKE_CODEX_COMMANDS", &self.commands_file)
            .env("FAKE_CODEX_ARGV", &self.argv_file)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        if let Some(answer) = answer {
            command.env("FAKE_CODEX_ANSWER", answer);
        }
        command
            .spawn()
            .expect("node is required for the fake Codex app-server")
    }

    /// Every request the fake child was given, in arrival order. A record file
    /// that was never written is an empty list, which is the answer a
    /// request-free command should produce.
    pub(super) fn recorded(&self) -> Vec<(String, serde_json::Value)> {
        let text = std::fs::read_to_string(&self.commands_file).unwrap_or_default();
        text.lines()
            .filter_map(|line| {
                let (method, params) = line.split_once('\t')?;
                Some((
                    method.to_string(),
                    serde_json::from_str(params).unwrap_or(serde_json::Value::Null),
                ))
            })
            .collect()
    }

    pub(super) fn methods(&self) -> Vec<String> {
        self.recorded()
            .into_iter()
            .map(|(method, _)| method)
            .collect()
    }

    /// The argv the fake child read, after the `-e <script> --` that started it.
    pub(super) fn argv(&self) -> Vec<String> {
        std::fs::read_to_string(&self.argv_file)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// A machine without node answers the reason, and the test says so and
    /// passes — the arrangement `codex_client_tests.rs` already uses.
    pub(super) fn skip_without_node() -> Option<String> {
        crate::test_support::external_program_skip_reason("node")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub(super) fn stdin_of(child: &mut std::process::Child) -> SharedStdin {
    Arc::new(Mutex::new(child.stdin.take()))
}

/// Wait for the fake child to answer `count` requests. Its answer on stdout is
/// what proves each request reached it and its record line was written —
/// reading the record file before this races a child that has not read its
/// stdin yet.
pub(super) fn await_answers(child: &mut std::process::Child, count: usize) {
    use std::io::{BufRead, BufReader};
    let stdout = child.stdout.take().expect("the fake child's stdout");
    let mut stdout = BufReader::new(stdout);
    for _ in 0..count {
        let mut line = String::new();
        stdout
            .read_line(&mut line)
            .expect("the fake child answers its requests");
    }
}

/// The thread the handshake answered with, and the catalog the existing Codex
/// tests use. `thread-fake` is the id every command request must name.
pub(super) fn thread_state() -> Arc<CodexState> {
    Arc::new(CodexState::new(
        "thread-fake".to_string(),
        catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog"),
        "auto",
    ))
}

pub(super) fn out_of_band_on(stdin: SharedStdin, commands: Arc<CodexCommands>) -> CodexOutOfBand {
    CodexOutOfBand::new(stdin, Arc::new(AtomicU64::new(1)), thread_state(), commands)
}

pub(super) fn writer_on(stdin: SharedStdin, commands: Arc<CodexCommands>) -> CodexWriter {
    CodexWriter {
        stdin,
        next_id: Arc::new(AtomicU64::new(1)),
        state: thread_state(),
        commands,
        pending: Vec::new(),
    }
}

/// One out-of-band text against a fresh fake child. The return is what the
/// shared seam published — `None` says the text was not claimed.
pub(super) fn run_out_of_band(
    fixture: &Fixture,
    commands: Arc<CodexCommands>,
    text: &str,
) -> Option<Vec<devboule_protocol::SessionEvent>> {
    let mut child = fixture.child(None);
    let stdin = stdin_of(&mut child);
    let handler = out_of_band_on(stdin, Arc::clone(&commands));
    if !handler.handles_out_of_band(text) {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    let runtime = Arc::new(SessionRuntime::new());
    runtime.stream.lock().unwrap().screen = None;
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.codex.command-test",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    handler.run_out_of_band(text, &runtime);
    // The fake child answers the request before it is stopped; this transport
    // test does not run a reader to publish that response.
    if commands
        .command(text)
        .and_then(|command| command.request("thread-fake"))
        .is_some()
    {
        await_answers(&mut child, 1);
    }
    let _ = child.kill();
    let _ = child.wait();
    Some(
        conn.pull_events()
            .into_iter()
            .map(|event| event.envelope.event)
            .collect(),
    )
}
