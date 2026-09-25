//! The Codex command tests against a fake app-server child: what reaches
//! Codex for `/compact` and `/goal`, what reaches it for a picked prompt or
//! skill, and what the launch line carries.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use devboule_protocol::{NoticeSeverity, SessionEvent};

use super::super::{OutOfBandCommands, StaticImageSink};
use super::command_test_support::{
    await_answers, out_of_band_on, run_out_of_band, stdin_of, thread_state, writer_on, Fixture,
    FAKE_CODEX,
};
use super::{spawn_codex, CodexRequests, CodexStaticPrompt, CodexSteerer, ThreadRoad};
use crate::attachment_store::AttachmentStore;
use crate::codex_goals::Goals;

/// A temp dir owning an [`AttachmentStore`], removed on drop — even when an
/// assertion panics — so a failed test leaks no directory.
struct StoreTempDir(std::path::PathBuf);

impl StoreTempDir {
    fn new(tag: &str) -> Self {
        Self(crate::test_dirs::test_temp_dir(&format!(
            "devboule-codex-{tag}"
        )))
    }

    fn store(&self) -> AttachmentStore {
        AttachmentStore::new(&self.0)
    }
}

impl Drop for StoreTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn plan_attachment(
    name: &str,
    mime_type: &str,
    bytes: &[u8],
) -> devboule_protocol::PromptAttachment {
    use base64::Engine;
    devboule_protocol::PromptAttachment {
        name: name.to_string(),
        mime_type: mime_type.to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

#[test]
fn compact_reaches_the_app_server_as_its_own_request_and_never_as_a_turn() {
    let Some(reason) = Fixture::skip_without_node() else {
        let fixture = Fixture::new("compact");
        let events = run_out_of_band(&fixture, fixture.commands(false, true), "/compact")
            .expect("/compact is handled out of band, before any turn is started");
        assert!(
            events.is_empty(),
            "an accepted compaction says nothing of its own: Codex reports it with \
             `thread/compacted`, which the reader turns into a notice"
        );
        assert_eq!(
            fixture.recorded(),
            vec![(
                "thread/compact/start".to_string(),
                serde_json::json!({ "threadId": "thread-fake" })
            )],
            "the request the app-server defines, naming this thread, and no turn"
        );
        return;
    };
    eprintln!("{reason}");
}

#[test]
fn every_goal_form_reaches_the_app_server_with_the_params_paseo_sends() {
    let Some(reason) = Fixture::skip_without_node() else {
        let cases = [
            (
                "/goal ship the fix",
                "thread/goal/set",
                serde_json::json!({
                    "threadId": "thread-fake",
                    "objective": "ship the fix",
                    "status": "active",
                }),
            ),
            (
                // Pause and resume carry no objective: Paseo's params have
                // none, and an empty string is not the same as an absent key.
                "/goal pause",
                "thread/goal/set",
                serde_json::json!({ "threadId": "thread-fake", "status": "paused" }),
            ),
            (
                "/goal resume",
                "thread/goal/set",
                serde_json::json!({ "threadId": "thread-fake", "status": "active" }),
            ),
            (
                "/goal clear",
                "thread/goal/clear",
                serde_json::json!({ "threadId": "thread-fake" }),
            ),
        ];
        for (text, method, params) in cases {
            let fixture = Fixture::new("goal-forms");
            let events = run_out_of_band(&fixture, fixture.commands(false, true), text)
                .expect("the goal forms are handled out of band");
            assert!(
                events.is_empty(),
                "a goal line waits for Codex's answer, so nothing is published yet"
            );
            assert_eq!(
                fixture.recorded(),
                vec![(method.to_string(), params)],
                "{text}: the one request it writes, with the params Paseo builds"
            );
        }
        return;
    };
    eprintln!("{reason}");
}

#[test]
fn a_bare_goal_writes_nothing_and_answers_the_usage_line() {
    let Some(reason) = Fixture::skip_without_node() else {
        let fixture = Fixture::new("goal-usage");
        let events = run_out_of_band(&fixture, fixture.commands(false, true), "/goal")
            .expect("the usage line is an answer, not a prompt");
        assert_eq!(
            events,
            vec![SessionEvent::SessionNotice {
                text: "Usage: /goal <objective>|pause|resume|clear".to_string(),
                severity: NoticeSeverity::Info,
            }],
            "Paseo returns this text before it touches the client (:5035-5037)"
        );
        assert!(
            fixture.recorded().is_empty(),
            "no request for a request-free answer"
        );
        return;
    };
    eprintln!("{reason}");
}

#[test]
fn goal_on_an_old_binary_is_not_intercepted_and_reaches_codex_as_text() {
    // Paseo's `tryHandleOutOfBand` returns null for `goal` when the gate failed
    // (:4997), and `resolveSlashCommandInvocation` then finds no `goal` in
    // `listCommands` (:4004-4021) — so the text goes out as the ordinary
    // `turn/start` it always was, verbatim and not as `$goal …`.
    let Some(reason) = Fixture::skip_without_node() else {
        let fixture = Fixture::new("old-goal");
        let commands = fixture.commands(false, false);
        let mut child = fixture.child(None);
        let stdin = stdin_of(&mut child);
        let handler = out_of_band_on(stdin.clone(), commands.clone());
        assert!(!handler.handles_out_of_band("/goal ship it"));
        drop(handler);
        let mut writer = writer_on(stdin);
        writer
            .write_all(b"/goal ship it")
            .expect("the text is written");
        writer.flush().expect("the turn goes out");
        await_answers(&mut child, 1);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(
            fixture.methods(),
            vec!["turn/start".to_string()],
            "an old binary gets the text as a prompt, which is what it can do with it"
        );
        assert_eq!(
            fixture.recorded()[0].1["input"][0]["text"],
            "/goal ship it",
            "and the text is not rewritten into a command form its goals are off for"
        );
        return;
    };
    eprintln!("{reason}");
}

#[test]
fn a_full_owed_table_tells_the_user_why_the_command_was_not_sent() {
    let fixture = Fixture::new("owed-cap");
    let commands = fixture.commands(false, true);
    let command = commands.command("/compact").expect("compact is available");
    for index in 0..32 {
        assert!(commands.owe(&format!("d-{index}"), &command));
    }
    let mut child = fixture.child(None);
    let stdin = stdin_of(&mut child);
    let handler = out_of_band_on(Arc::clone(&stdin), Arc::clone(&commands));
    let runtime = Arc::new(super::super::SessionRuntime::new());
    runtime.stream.lock().unwrap().screen = None;
    let conn = super::super::event_pull::ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.codex.command-cap",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    handler.run_out_of_band("/compact", &runtime);
    // The fake child answers requests in input order. A barrier request proves
    // it has consumed every preceding write before the record file is checked.
    stdin
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":\"barrier\",\"method\":\"barrier\",\"params\":{}}\n",
        )
        .expect("write the child barrier");
    await_answers(&mut child, 1);
    let recorded = fixture.recorded();
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        recorded.iter().all(|(method, _)| method == "barrier"),
        "the untracked command is not sent"
    );
    let events: Vec<SessionEvent> = conn
        .pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .collect();
    assert_eq!(
        events,
        vec![SessionEvent::SessionNotice {
            text: "The Codex command was not sent because its response could not be tracked. Retry it.".to_string(),
            severity: NoticeSeverity::Warning,
        }]
    );
}

#[test]
fn a_picked_command_is_refused_as_a_steer() {
    let fixture = Fixture::new("steer-command");
    let steerer = CodexSteerer {
        stdin: std::sync::Arc::new(std::sync::Mutex::new(None)),
        next_id: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)),
        state: {
            let state = super::command_test_support::thread_state();
            state.set_turn(Some("turn-3".to_string()));
            state
        },
        requests: std::sync::Arc::new(CodexRequests::new()),
        commands: fixture.commands(true, false),
    };
    let mut steerer = steerer;
    assert!(matches!(
        crate::test_support::steer_through_the_turn(&mut steerer, "/plotting sales.csv"),
        Some(Ok(false))
    ));
}

#[test]
fn the_writer_sends_text_literally_and_leaves_commands_to_the_static_route() {
    // The writer's one job is writing text: a picked command that reaches it
    // directly goes out as typed, still a turn. Expansion lives one level up,
    // in the static plan the send path always consults first — a picked
    // command never reaches this writer in production, so the writer must not
    // guess which trailing section of a composed prompt is one.
    let Some(reason) = Fixture::skip_without_node() else {
        let fixture = Fixture::new("picked");
        let mut child = fixture.child(None);
        let stdin = stdin_of(&mut child);
        let mut writer = writer_on(stdin);
        for text in [
            "/prompts:commit stage",
            "/plotting sales.csv",
            "/unknown thing",
        ] {
            writer
                .write_all(text.as_bytes())
                .expect("a prompt is written");
            writer.flush().expect("the turn goes out");
        }
        await_answers(&mut child, 3);
        let _ = child.kill();
        let _ = child.wait();
        let seen = fixture.recorded();
        assert_eq!(
            seen.iter()
                .map(|(method, _)| method.as_str())
                .collect::<Vec<_>>(),
            ["turn/start", "turn/start", "turn/start"],
            "every text is still a turn"
        );
        for (recorded, text) in seen.iter().zip([
            "/prompts:commit stage",
            "/plotting sales.csv",
            "/unknown thing",
        ]) {
            assert_eq!(
                recorded.1["input"],
                serde_json::json!([{ "type": "text", "text": text }]),
                "the writer expands nothing: {text}"
            );
        }
        return;
    };
    eprintln!("{reason}");
}

#[test]
fn a_composed_first_command_travels_expanded_on_the_static_route() {
    // The composed-expansion seam both wiring lines serve:
    // `CodexStaticPrompt::plan_prompt` resolves the picked command against
    // the user's message, and `CodexPlannedPrompt::params` puts the expanded
    // blocks on the wire. Deleting either line sends the literal slash line.
    let Some(reason) = Fixture::skip_without_node() else {
        let fixture = Fixture::new("static-wire");
        let commands = fixture.commands(true, false);
        let mut child = fixture.child(None);
        let stdin = stdin_of(&mut child);
        let route =
            CodexStaticPrompt::new(stdin, Arc::new(AtomicU64::new(1)), thread_state(), commands);
        let store_dir = StoreTempDir::new("static-wire");
        let store = store_dir.store();
        let raw = "/prompts:commit stage";
        let composed = format!("standing instructions\n\nspawn prompt\n\n{raw}");
        let plan = route
            .plan_prompt(&store, "s.codex.static-wire", &composed, raw, &[])
            .expect("planning runs")
            .expect("a picked command is claimed by the static route");
        plan.send().expect("the turn goes out");
        await_answers(&mut child, 1);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(
            fixture.methods(),
            vec!["turn/start".to_string()],
            "a picked command is still a turn"
        );
        assert_eq!(
            fixture.recorded()[0].1["input"],
            serde_json::json!([{ "type": "text", "text": "standing instructions\n\nspawn prompt\n\nOn stage: stage\n" }]),
            "the first-turn prefix survives command expansion on the wire"
        );
        return;
    };
    eprintln!("{reason}");
}

#[test]
fn a_composed_command_with_an_svg_attachment_expands_around_its_path_line() {
    // The attachments-branch wiring: the plan resolves the command against
    // the message while the SVG keeps its path line in the journal text.
    // Deleting the plan-time assignment sends the literal slash line with
    // the SVG path instead of the expanded body. The SVG line itself stays
    // out of the input blocks — the recorded SVG P3, pinned deliberately.
    let Some(reason) = Fixture::skip_without_node() else {
        let fixture = Fixture::new("static-svg");
        let commands = fixture.commands(true, false);
        let mut child = fixture.child(None);
        let stdin = stdin_of(&mut child);
        let route =
            CodexStaticPrompt::new(stdin, Arc::new(AtomicU64::new(1)), thread_state(), commands);
        let store_dir = StoreTempDir::new("static-svg");
        let store = store_dir.store();
        let raw = "/prompts:commit stage";
        let composed = format!("standing instructions\n\n{raw}");
        let plan = route
            .plan_prompt(
                &store,
                "s.codex.static-svg",
                &composed,
                raw,
                &[plan_attachment(
                    "drawing.svg",
                    "image/svg+xml",
                    b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
                )],
            )
            .expect("planning runs")
            .expect("a picked command with an attachment is planned");
        assert!(
            plan.text().starts_with(&composed),
            "the journal text opens with the composed prompt"
        );
        assert!(
            plan.text().ends_with(".svg]"),
            "and closes with the SVG path line: {}",
            plan.text()
        );
        plan.send().expect("the turn goes out");
        await_answers(&mut child, 1);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(
            fixture.recorded()[0].1["input"],
            serde_json::json!([{ "type": "text", "text": "standing instructions\n\nOn stage: stage\n" }]),
            "the wire carries the expanded body; the SVG line stays in the journal"
        );
        return;
    };
    eprintln!("{reason}");
}

#[test]
fn a_picked_skill_with_stored_references_sends_blocks_and_paths() {
    // The writer-path P2: a picked command next to stored references went out
    // literally on a first prompt, and with the path lines glued into its
    // arguments on later ones — while the real paths never reached Codex.
    // The static route claims the command and appends the reference lines to
    // the blocks, so the expanded text and the paths both travel.
    let Some(reason) = Fixture::skip_without_node() else {
        let fixture = Fixture::new("static-refs");
        let commands = fixture.commands(true, false);
        let mut child = fixture.child(None);
        let stdin = stdin_of(&mut child);
        let route =
            CodexStaticPrompt::new(stdin, Arc::new(AtomicU64::new(1)), thread_state(), commands);
        let store_dir = StoreTempDir::new("static-refs");
        let store = store_dir.store();
        let raw = "/plotting sales.csv";
        let composed = format!("standing instructions\n\n{raw}");
        let mut plan = route
            .plan_prompt(&store, "s.codex.static-refs", &composed, raw, &[])
            .expect("planning runs")
            .expect("a picked skill is claimed by the static route");
        let deck = PathBuf::from("/deck/sales.pdf");
        plan.append_reference_path_lines(std::slice::from_ref(&deck));
        assert_eq!(
            plan.text(),
            format!("{composed}\n\n[Image available at: /deck/sales.pdf]"),
            "the journal text carries the composed prompt and the path"
        );
        plan.send().expect("the turn goes out");
        await_answers(&mut child, 1);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(
            fixture.recorded()[0].1["input"],
            serde_json::json!([
                { "type": "text", "text": "standing instructions\n\n" },
                { "type": "skill", "name": "plotting", "path": fixture.cwd().join(".codex").join("skills").join("plotting").join("SKILL.md") },
                { "type": "text", "text": "$plotting sales.csv\n\n[Image available at: /deck/sales.pdf]" },
            ]),
            "the skill blocks travel expanded, and the reference path with them"
        );
        return;
    };
    eprintln!("{reason}");
}

#[test]
fn a_later_paragraph_naming_a_command_is_not_one_on_any_route() {
    // The suffix P3: Paseo expands only the message that IS the command
    // (`parseSlashCommandInput` :3986-4001 on the whole prompt). A message
    // whose last paragraph merely names a listed command goes out literally,
    // whether or not it was composed around.
    let Some(reason) = Fixture::skip_without_node() else {
        let fixture = Fixture::new("suffix");
        let commands = fixture.commands(true, false);
        let store_dir = StoreTempDir::new("suffix");
        let store = store_dir.store();
        let mut child = fixture.child(None);
        let stdin = stdin_of(&mut child);
        let route = CodexStaticPrompt::new(
            stdin.clone(),
            Arc::new(AtomicU64::new(1)),
            thread_state(),
            Arc::clone(&commands),
        );
        let text = "context\n\n/plotting sales.csv";
        assert!(
            route
                .plan_prompt(&store, "s.codex.suffix", text, text, &[])
                .expect("planning runs")
                .is_none(),
            "the static route claims no suffix command"
        );
        let mut writer = writer_on(stdin);
        writer.write_all(text.as_bytes()).expect("written");
        writer.flush().expect("the turn goes out");
        await_answers(&mut child, 1);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(
            fixture.recorded()[0].1["input"],
            serde_json::json!([{ "type": "text", "text": text }]),
            "the writer sends the paragraphs as typed"
        );
        return;
    };
    eprintln!("{reason}");
}

#[test]
fn the_writer_keeps_builtin_compact_text_unchanged() {
    // This writer-only check pins that a built-in is never expanded as a
    // custom prompt. Attachment bypass is exercised at the session boundary.
    let Some(reason) = Fixture::skip_without_node() else {
        let fixture = Fixture::new("attached");
        let mut child = fixture.child(None);
        let stdin = stdin_of(&mut child);
        let mut writer = writer_on(stdin);
        writer
            .write_all(b"/compact with the picture")
            .expect("the text is written");
        writer.flush().expect("the turn goes out");
        await_answers(&mut child, 1);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(
            fixture.methods(),
            vec!["turn/start".to_string()],
            "the writer never runs the out-of-band hook: the send path does, and \
             it declines a prompt carrying attachments"
        );
        assert_eq!(
            fixture.recorded()[0].1["input"][0]["text"],
            "/compact with the picture",
            "and the text is left as typed rather than rewritten to $compact"
        );
        return;
    };
    eprintln!("{reason}");
}

#[test]
fn launch_args_carry_the_goals_flag_only_when_the_gate_passes() {
    let Some(reason) = Fixture::skip_without_node() else {
        for (version, expected) in [
            ("codex-cli 0.155.1", true),
            ("codex-cli 0.128.0", true),
            ("codex-cli 0.127.9", false),
            ("codex-cli 0.9.4", false),
        ] {
            let fixture = Fixture::new("argv");
            let goals = Goals::from_version_output(version);
            let state = crate::server::ServerState::new("codex-argv-gate".to_string());
            let command = crate::session::PtyCommand::new(
                "node",
                vec!["-e".to_string(), FAKE_CODEX.to_string(), "--".to_string()],
                fixture.cwd(),
                vec![(
                    "FAKE_CODEX_ARGV".to_string(),
                    fixture.argv_file().to_string_lossy().into_owned(),
                )],
            );
            let mut spawned = spawn_codex(
                &state,
                command,
                None,
                crate::profile_delivery::ProfileDelivery::none(),
                ThreadRoad::Fresh,
                goals,
                fixture.commands(false, goals.enabled()),
            )
            .unwrap_or_else(|error| panic!("{version} must spawn: {}", error.message));
            let argv = fixture.argv();
            spawned.killer.kill();
            // Node's `-e <script> --` leaves argv[1..] as exactly the flags the
            // launcher appended, so this is the whole tail of the real launch
            // line — the pair `spawnAppServer` appends (:7089-7091), in order.
            if expected {
                assert_eq!(
                    argv,
                    vec!["--enable".to_string(), "goals".to_string()],
                    "the gate read {version}"
                );
            } else {
                assert!(
                    argv.is_empty(),
                    "the gate read {version}, so nothing is appended: {argv:?}"
                );
            }
        }
        return;
    };
    eprintln!("{reason}");
}
