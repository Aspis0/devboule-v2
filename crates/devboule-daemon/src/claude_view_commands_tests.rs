//! Tests for one topic: the slash-command menu — the init frame's names,
//! the initialize handshake's rich list, their merge and dedup, and the
//! bounds both parses share.

use devboule_protocol::SessionEvent;
use serde_json::{json, Value};

use crate::claude_view::test_support::{init_frame, view};
use crate::claude_view::ClaudeView;

/// The shape of a real init envelope with its command list, invented
/// values throughout: every key the measured init
/// carries, none of the probe's paths, user names, project names or
/// session ids. `slash_commands` is a flat array of names — no
/// descriptions, no hints — and `terminal_slash_commands` is a separate
/// list this daemon does not publish.
fn init_frame_with_slash_commands() -> Value {
    json!({
        "type": "system",
        "subtype": "init",
        "cwd": r"C:\work\sample-project",
        "session_id": "00000000-0000-4000-8000-000000000001",
        "tools": ["Bash", "Read", "Edit"],
        "mcp_servers": [],
        "model": "claude-test-model",
        "permissionMode": "default",
        "slash_commands": ["clear", "compact", "autocompact", "model", "usage"],
        "terminal_slash_commands": ["doctor", "reload-plugins"],
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
    })
}

#[test]
fn system_init_publishes_the_slash_commands_it_carries() {
    let mut mapper = view();
    let events = mapper.ingest(&init_frame_with_slash_commands());
    match events.as_slice() {
        [SessionEvent::SessionManifest { .. }, SessionEvent::AvailableCommands { commands }] => {
            let names = commands
                .iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(
                names,
                ["clear", "compact", "autocompact", "model", "usage"],
                "the list is the init's own array, in its own order"
            );
            assert!(
                commands
                    .iter()
                    .all(|command| command.description.is_empty() && command.hint.is_none()),
                "a flat name array carries no descriptions and no hints"
            );
        }
        other => panic!("expected manifest then commands, got {other:?}"),
    }
}

/// The companion guard: an init without the field keeps publishing the
/// manifest it has always published and adds no command list (green by
/// construction — its sibling above is the red one).
#[test]
fn an_init_without_slash_commands_publishes_no_command_list() {
    let mut mapper = view();
    let events = mapper.ingest(&init_frame());
    assert!(
        matches!(events.as_slice(), [SessionEvent::SessionManifest { .. }]),
        "nothing changes for an init that carries no commands: {events:?}"
    );
}

#[test]
fn the_init_command_list_copies_at_most_a_thousand_names() {
    // A correctly typed but enormous array must not become an enormous
    // event: the first thousand names are copied into
    // the view and the rest dropped.
    let mut envelope = init_frame_with_slash_commands();
    let names = (0..1005)
        .map(|index| format!("cmd{index}"))
        .collect::<Vec<_>>();
    envelope["slash_commands"] = serde_json::json!(names);
    let mut mapper = view();
    let events = mapper.ingest(&envelope);
    match events.as_slice() {
        [SessionEvent::SessionManifest { .. }, SessionEvent::AvailableCommands { commands }] => {
            assert_eq!(commands.len(), 1000, "the bound, and only the bound")
        }
        other => panic!("expected manifest then a capped list, got {other:?}"),
    }
}

/// The initialize handshake's answer, shaped as the SDK's initialize
/// response carries `commands` (name, description, argumentHint):
/// invented values throughout — no probe path, name, or id.
fn initialize_response() -> Value {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": "initial-commands-9",
            "response": {
                "commands": [
                    {"name": "clear", "description": "Clear the transcript", "argumentHint": ""},
                    {"name": "compact", "description": "Compact the context", "argumentHint": "[instructions]"},
                    {"name": "usage", "description": "Show usage", "argumentHint": "<detail>", "aliases": ["cost"], "builtin": true}
                ]
            }
        }
    })
}

#[test]
fn initialize_response_publishes_commands_with_descriptions_and_hints() {
    // The handshake fills the menu before the first prompt: names with
    // their descriptions and argument hints — the init frame's flat names
    // carry neither.
    let mut mapper = ClaudeView::new(None);
    match mapper.ingest(&initialize_response()).as_slice() {
        [SessionEvent::AvailableCommands { commands }] => {
            let listed = commands
                .iter()
                .map(|command| {
                    (
                        command.name.as_str(),
                        command.description.as_str(),
                        command.hint.as_deref(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                listed,
                [
                    ("clear", "Clear the transcript", None),
                    ("compact", "Compact the context", Some("[instructions]")),
                    ("usage", "Show usage", Some("<detail>")),
                ]
            );
        }
        other => panic!("expected one command list, got {other:?}"),
    }
}

#[test]
fn init_after_initialize_publishes_no_second_list() {
    // One consistent list: the init frame's names match what the
    // handshake already published, so only the manifest is emitted —
    // a reattach replays the same rows in the same order and derives
    // the same single list.
    let mut mapper = ClaudeView::new(None);
    let _ = mapper.ingest(&initialize_response());
    let mut init = init_frame();
    init["slash_commands"] = json!(["clear", "compact", "usage"]);
    let events = mapper.ingest(&init);
    assert!(
        matches!(events.as_slice(), [SessionEvent::SessionManifest { .. }]),
        "the init's names repeat the handshake list: {events:?}"
    );
}

#[test]
fn init_with_new_names_merges_over_the_handshake_list() {
    // Union, not intersection: the handshake's entries first with their
    // words, then init names it never listed, appended bare in init order.
    // The wire order stays, unsorted.
    let mut mapper = ClaudeView::new(None);
    let _ = mapper.ingest(&initialize_response());
    let events = mapper.ingest(&init_frame_with_slash_commands());
    match events.as_slice() {
        [SessionEvent::SessionManifest { .. }, SessionEvent::AvailableCommands { commands }] => {
            let listed = commands
                .iter()
                .map(|command| {
                    (
                        command.name.as_str(),
                        command.description.as_str(),
                        command.hint.as_deref(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                listed,
                [
                    ("clear", "Clear the transcript", None),
                    ("compact", "Compact the context", Some("[instructions]")),
                    ("usage", "Show usage", Some("<detail>")),
                    ("autocompact", "", None),
                    ("model", "", None),
                ],
                "the handshake's entries, then the init-only names bare"
            );
            assert!(
                !commands.iter().any(|command| command.name == "rewind"),
                "no synthesized rewind: there is no native rewind road here to back one with"
            );
        }
        other => panic!("expected manifest then a merged list, got {other:?}"),
    }
}

#[test]
fn handshake_names_the_init_omits_stay_on_the_menu() {
    // The other direction of the union: an init carrying only `clear`
    // republishes nothing, so the handshake's `compact` and `usage` stay
    // listed with their words.
    let mut mapper = ClaudeView::new(None);
    let _ = mapper.ingest(&initialize_response());
    let mut init = init_frame();
    init["slash_commands"] = json!(["clear"]);
    let events = mapper.ingest(&init);
    assert!(
        matches!(events.as_slice(), [SessionEvent::SessionManifest { .. }]),
        "the init omits names but deletes none: {events:?}"
    );
}

#[test]
fn a_failed_initialize_leaves_no_list() {
    // An older CLI's error, like its silence, only costs the early list:
    // the init frame stays the list's source.
    let mut mapper = ClaudeView::new(None);
    let refused = json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": "initial-commands-9",
            "error": "Unknown control request subtype: initialize"
        }
    });
    assert!(mapper.ingest(&refused).is_empty());
    match mapper.ingest(&init_frame_with_slash_commands()).as_slice() {
        [SessionEvent::SessionManifest { .. }, SessionEvent::AvailableCommands { commands }] => {
            assert_eq!(commands.len(), 5, "the init's names, bare as ever");
        }
        other => panic!("expected manifest then commands, got {other:?}"),
    }
}

#[test]
fn initialize_entries_past_the_inspection_bound_are_never_parsed() {
    // Same reader-thread bound as the reply and the init: the valid
    // entry behind ten thousand malformed ones is not listed.
    let mut entries: Vec<Value> = (0..10_005).map(|_| json!({})).collect();
    entries.push(json!({ "name": "goal", "description": "Set the goal" }));
    let response = json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": "initial-commands-9",
            "response": { "commands": entries }
        }
    });
    let mut mapper = ClaudeView::new(None);
    match mapper.ingest(&response).as_slice() {
        [SessionEvent::AvailableCommands { commands }] => {
            assert!(commands.is_empty(), "nothing accepted past the bound");
        }
        other => panic!("expected one command list, got {other:?}"),
    }
}

#[test]
fn init_names_past_the_inspection_bound_are_never_parsed() {
    let mut envelope = init_frame_with_slash_commands();
    let mut names: Vec<Value> = (0..10_005).map(|index| json!(index)).collect();
    names.push(json!("goal"));
    envelope["slash_commands"] = Value::Array(names);
    let mut mapper = view();
    let events = mapper.ingest(&envelope);
    match events.as_slice() {
        [SessionEvent::SessionManifest { .. }, SessionEvent::AvailableCommands { commands }] => {
            assert!(
                !commands.iter().any(|command| command.name == "goal"),
                "the name behind the flood is not listed"
            );
        }
        other => panic!("expected manifest then a capped list, got {other:?}"),
    }
}

#[test]
fn initialize_commands_skip_malformed_entries_without_spending_the_bound() {
    // The bound counts accepted entries, like the init's: a thousand
    // malformed entries first must not crowd out the valid ones.
    let mut entries: Vec<Value> = (0..1005).map(|_| json!({})).collect();
    entries
        .push(json!({ "name": "review", "description": "Review the work", "argumentHint": "<o>" }));
    entries.push(json!({ "name": "" }));
    let response = json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": "initial-commands-9",
            "response": { "commands": entries }
        }
    });
    let mut mapper = ClaudeView::new(None);
    match mapper.ingest(&response).as_slice() {
        [SessionEvent::AvailableCommands { commands }] => {
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0].name, "review");
            assert_eq!(commands[0].hint.as_deref(), Some("<o>"));
        }
        other => panic!("expected one command list, got {other:?}"),
    }
}
