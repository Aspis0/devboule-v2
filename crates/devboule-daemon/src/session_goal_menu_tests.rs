//! Provider-advertised `goal` entries never reach the published command menu.
//! One phrase for the file — command filtering — beside the intercept
//! (`session_goal_tests.rs`), dispatch (`session_goal_dispatch_tests.rs`),
//! and the durable half (`session_goal_journal_tests.rs`).

use devboule_protocol::SessionEvent;

use super::is_reserved_goal_command;

#[test]
fn provider_advertised_goal_commands_never_reach_the_menu() {
    assert!(is_reserved_goal_command("goal"));
    assert!(!is_reserved_goal_command("goals"));
    assert!(!is_reserved_goal_command("compact"));

    // pi's `get_commands` reply, with an extension-advertised `goal` beside
    // an ordinary command: the menu keeps the latter only.
    let reply: serde_json::Value = serde_json::from_str(
        r#"{"id":"c-g","type":"response","command":"get_commands","success":true,"data":{"commands":[{"name":"goal","description":"extension goal","source":"pi-goal-x"},{"name":"review","description":"review it"}]}}"#,
    )
    .expect("reply");
    match crate::pi_view::events_from_line(&reply).as_slice() {
        [SessionEvent::AvailableCommands { commands }] => {
            assert!(
                commands.iter().all(|command| command.name != "goal"),
                "the advertised goal is dropped: {:?}",
                commands
                    .iter()
                    .map(|command| &command.name)
                    .collect::<Vec<_>>()
            );
            assert!(
                commands.iter().any(|command| command.name == "review"),
                "ordinary commands still list"
            );
        }
        other => panic!("expected one command list, got {other:?}"),
    }

    // ACP's `available_commands_update`, same rule.
    let line: serde_json::Value = serde_json::from_str(
        r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"goal","description":"agent goal"},{"name":"compact","description":"compact it","input":{"hint":"x"}}]}}}"#,
    )
    .expect("update");
    match crate::acp_view::view_from_envelope(&line, "s").as_slice() {
        [SessionEvent::AvailableCommands { commands }] => {
            assert!(
                commands.iter().all(|command| command.name != "goal"),
                "the advertised goal is dropped"
            );
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0].name, "compact");
        }
        other => panic!("expected one command list, got {other:?}"),
    }

    // Claude's handshake list, same rule.
    let response = serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "response": { "commands": [
                {"name": "goal", "description": "Set the goal"},
                {"name": "clear", "description": "Clear the transcript"},
            ] }
        }
    });
    let mut mapper = crate::claude_view::ClaudeView::new(None);
    match mapper.ingest(&response).as_slice() {
        [SessionEvent::AvailableCommands { commands }] => {
            assert!(
                commands.iter().all(|command| command.name != "goal"),
                "the advertised goal is dropped"
            );
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0].name, "clear");
        }
        other => panic!("expected one command list, got {other:?}"),
    }

    // Claude's `slash_commands` array, same rule: a flat name list carrying
    // `goal` beside ordinary names publishes the latter only.
    let init = serde_json::json!({
        "type": "system",
        "subtype": "init",
        "cwd": r"C:\work\sample-project",
        "session_id": "00000000-0000-4000-8000-000000000001",
        "tools": ["Bash"],
        "mcp_servers": [],
        "model": "claude-test-model",
        "permissionMode": "default",
        "slash_commands": ["goal", "clear"],
        "apiKeySource": "test",
        "claude_code_version": "0.0.0-test",
        "output-style": "default",
        "agents": [],
        "skills": [],
        "plugins": [],
        "capabilities": {},
        "analytics_disabled": false,
        "uuid": "00000000-0000-4000-8000-000000000002",
        "memory_paths": [],
        "messaging_socket_path": "",
        "fast_mode_state": "disabled",
        "fast_mode_disabled_reason": null,
        "powershell_path": ""
    });
    let mut mapper = crate::claude_view::ClaudeView::new(None);
    match mapper.ingest(&init).as_slice() {
        [SessionEvent::SessionManifest { .. }, SessionEvent::AvailableCommands { commands }] => {
            assert!(
                commands.iter().all(|command| command.name != "goal"),
                "the flat array drops the advertised goal"
            );
            assert!(
                commands.iter().any(|command| command.name == "clear"),
                "ordinary names still list"
            );
        }
        other => panic!("expected manifest then commands, got {other:?}"),
    }
}
