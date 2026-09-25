//! The Codex command tests against a fake app-server child: what reaches
//! Codex for `/compact` and `/goal`, what reaches it for a picked prompt or
//! skill, and what the launch line carries.

use std::io::Write;
use std::sync::Arc;

use devboule_protocol::{NoticeSeverity, SessionEvent};

use super::super::OutOfBandCommands;
use super::command_test_support::{
    await_answers, out_of_band_on, run_out_of_band, stdin_of, writer_on, Fixture, FAKE_CODEX,
};
use super::{spawn_codex, CodexRequests, CodexSteerer, ThreadRoad};
use crate::codex_goals::Goals;

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
        let mut writer = writer_on(stdin, commands);
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
    let handler = out_of_band_on(stdin_of(&mut child), Arc::clone(&commands));
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
    let _ = child.kill();
    let _ = child.wait();
    let events: Vec<SessionEvent> = conn
        .pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .collect();
    assert_eq!(
        events,
        vec![SessionEvent::SessionNotice {
            text: "Could not track the Codex command response; retry the command.".to_string(),
            severity: NoticeSeverity::Warning,
        }]
    );
    assert!(
        fixture.recorded().is_empty(),
        "the untracked command is not sent"
    );
}

#[test]
fn a_picked_command_is_refused_as_a_steer() {
    let fixture = Fixture::new("steer-command");
    let steerer = CodexSteerer {
        stdin: std::sync::Arc::new(std::sync::Mutex::new(None)),
        next_id: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)),
        state: super::command_test_support::thread_state(),
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
fn a_picked_prompt_or_skill_command_changes_the_turn_text_and_stays_a_turn() {
    let Some(reason) = Fixture::skip_without_node() else {
        let fixture = Fixture::new("picked");
        let commands = fixture.commands(true, false);
        let mut child = fixture.child(None);
        let stdin = stdin_of(&mut child);
        let mut writer = writer_on(stdin, commands);
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
            "a picked command is still a turn, and so is an unknown slash"
        );
        assert_eq!(
            seen[0].1["input"][0]["text"], "On stage: stage\n",
            "the custom prompt travels expanded, front matter and trailing newline \
             included, as Paseo builds it (:4030-4037, :656)"
        );
        assert_eq!(
            seen[1].1["input"],
            serde_json::json!([
                { "type": "skill", "name": "plotting", "path": fixture.cwd().join(".codex").join("skills").join("plotting").join("SKILL.md") },
                { "type": "text", "text": "$plotting sales.csv" },
            ]),
            "Paseo's populated-cache form carries the skill path and text (:4044-4052)"
        );
        assert_eq!(
            seen[2].1["input"][0]["text"], "/unknown thing",
            "a name the table does not carry is left exactly as typed"
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
        let commands = fixture.commands(true, true);
        let mut child = fixture.child(None);
        let stdin = stdin_of(&mut child);
        let mut writer = writer_on(stdin, commands);
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
