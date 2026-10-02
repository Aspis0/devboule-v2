//! Tests for one topic: the `command` and `exit_code` a tool row carries on
//! the ACP road — read from `rawInput`/`rawOutput` only for shell rows, and
//! derived identically by the replay road.
//!
//! No capture in `fixtures/wire` carries `rawInput`/`rawOutput`, so every
//! envelope below is shaped from the ACP v2 `ToolCallUpdate` schema.

use devboule_protocol::SessionEvent;

use super::view_from_envelope;
use crate::claude_view::{drive_replay, ClaudeView};

const SESSION: &str = "01a06c70-ea2b-7882-ad27-aae8188fc243";

fn parse(line: &str) -> serde_json::Value {
    serde_json::from_str(line).expect("probe json")
}

/// The one view of an envelope that models to exactly one event.
fn one_view(views: Vec<SessionEvent>) -> SessionEvent {
    let mut views = views;
    assert_eq!(views.len(), 1, "expected exactly one view, got {views:?}");
    views.pop().expect("one view")
}

fn tool_call(extra: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"{SESSION}","update":{{"sessionUpdate":"tool_call","toolCallId":"call-1","title":"Run tests","status":"in_progress"{extra}}}}}}}"#
    )
}

fn tool_call_update(extra: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"{SESSION}","update":{{"sessionUpdate":"tool_call_update","toolCallId":"call-1"{extra}}}}}}}"#
    )
}

#[test]
fn an_execute_tool_call_carries_the_command_it_was_sent() {
    let line = parse(&tool_call(
        r#","kind":"execute","rawInput":{"command":"cargo","args":["test","-p","devboule-daemon"]}"#,
    ));
    match view_from_envelope(&line, SESSION).as_slice() {
        [SessionEvent::AgentToolCall {
            command, exit_code, ..
        }] => {
            assert_eq!(command.as_deref(), Some("cargo test -p devboule-daemon"));
            assert_eq!(*exit_code, None, "a running call has reported no code");
        }
        other => panic!("expected one tool call, got {other:?}"),
    }

    // A `rawInput` without a command line, or none at all, carries no line:
    // `title` stays display text and is never promoted into `command`.
    for extra in [
        r#","kind":"execute","rawInput":{"cwd":"C:\\work"}"#,
        r#","kind":"execute""#,
        r#","rawInput":{"command":"cargo"}"#,
    ] {
        let line = parse(&tool_call(extra));
        match view_from_envelope(&line, SESSION).as_slice() {
            [SessionEvent::AgentToolCall { command, .. }] => {
                assert_eq!(*command, None, "extra {extra} names no command line");
            }
            other => panic!("expected one tool call, got {other:?}"),
        }
    }
}

#[test]
fn an_execute_update_carries_only_an_explicitly_numeric_exit_code() {
    // This view holds no row state: a patch is read as it states itself, and
    // the row merge, which holds the kind, qualifies these fields.
    for extra in [
        r#","kind":"execute","status":"completed","rawOutput":{"exitCode":0}"#,
        r#","status":"completed","rawOutput":{"exitCode":0}"#,
    ] {
        let line = parse(&tool_call_update(extra));
        match view_from_envelope(&line, SESSION).as_slice() {
            [SessionEvent::AgentToolUpdate {
                command, exit_code, ..
            }] => {
                assert_eq!(*exit_code, Some(0), "extra {extra}");
                assert_eq!(*command, None, "the update supplied no command line");
            }
            other => panic!("expected one tool update, got {other:?}"),
        }
    }

    // A failed status is not a code, and neither is a code sent as text:
    // only an explicit number in `rawOutput.exitCode` fills the field.
    for extra in [
        r#","kind":"execute","status":"failed","rawOutput":{"output":"boom"}"#,
        r#","kind":"execute","status":"failed","rawOutput":{"exitCode":"2"}"#,
        r#","kind":"execute","status":"failed""#,
    ] {
        let line = parse(&tool_call_update(extra));
        match view_from_envelope(&line, SESSION).as_slice() {
            [SessionEvent::AgentToolUpdate { exit_code, .. }] => {
                assert_eq!(*exit_code, None, "extra {extra} states no numeric code");
            }
            other => panic!("expected one tool update, got {other:?}"),
        }
    }
}

#[test]
fn an_exit_code_keeps_its_sign_and_stops_at_i32() {
    // The boundary is the i32 range: the sign is kept, and a fractional or
    // out-of-range number is no code this field can carry.
    for (extra, expected) in [
        (
            r#","kind":"execute","status":"completed","rawOutput":{"exitCode":-1}"#,
            Some(-1),
        ),
        (
            r#","kind":"execute","status":"completed","rawOutput":{"exitCode":-2147483649}"#,
            None,
        ),
        (
            r#","kind":"execute","status":"completed","rawOutput":{"exitCode":1.5}"#,
            None,
        ),
        (
            r#","kind":"execute","status":"completed","rawOutput":{"exitCode":2147483648}"#,
            None,
        ),
    ] {
        let line = parse(&tool_call_update(extra));
        match view_from_envelope(&line, SESSION).as_slice() {
            [SessionEvent::AgentToolUpdate { exit_code, .. }] => {
                assert_eq!(*exit_code, expected, "extra {extra}");
            }
            other => panic!("expected one tool update, got {other:?}"),
        }
    }
}

#[test]
fn a_row_of_any_other_kind_carries_no_command_and_no_exit_code() {
    for envelope in [
        tool_call(
            r#","kind":"read","rawInput":{"command":"cat src/lib.rs"},"rawOutput":{"exitCode":0}"#,
        ),
        tool_call_update(
            r#","kind":"read","status":"completed","rawInput":{"command":"cat src/lib.rs"},"rawOutput":{"exitCode":0}"#,
        ),
    ] {
        let line = parse(&envelope);
        match view_from_envelope(&line, SESSION).as_slice() {
            [SessionEvent::AgentToolCall {
                command, exit_code, ..
            }]
            | [SessionEvent::AgentToolUpdate {
                command, exit_code, ..
            }] => {
                assert_eq!(*command, None, "{envelope}");
                assert_eq!(*exit_code, None, "{envelope}");
            }
            other => panic!("expected one tool row, got {other:?}"),
        }
    }
}

#[test]
fn a_kindless_first_frame_reads_nothing_and_a_kindless_patch_reads_its_own_fields() {
    // The first announcement carries its own kind or none: with none there is
    // no row kind to hang a shell field on, so the fields stay empty.
    let first = parse(&tool_call(
        r#","rawInput":{"command":"cargo"},"rawOutput":{"exitCode":0}"#,
    ));
    match view_from_envelope(&first, SESSION).as_slice() {
        [SessionEvent::AgentToolCall {
            command, exit_code, ..
        }] => {
            assert_eq!(*command, None);
            assert_eq!(*exit_code, None);
        }
        other => panic!("expected one tool call, got {other:?}"),
    }

    // An absent `kind` is "unchanged", not "no shell row": the patch's own
    // fields are read here, and the row merge qualifies them.
    let patch = parse(&tool_call_update(r#","rawOutput":{"exitCode":7}"#));
    match view_from_envelope(&patch, SESSION).as_slice() {
        [SessionEvent::AgentToolUpdate { exit_code, .. }] => {
            assert_eq!(*exit_code, Some(7));
        }
        other => panic!("expected one tool update, got {other:?}"),
    }
}

#[test]
fn the_replay_road_derives_the_same_command_row() {
    for envelope in [
        tool_call(r#","kind":"execute","rawInput":{"command":"cargo","args":["test"]}"#),
        tool_call_update(r#","kind":"execute","status":"completed","rawOutput":{"exitCode":0}"#),
        tool_call_update(r#","status":"completed","rawOutput":{"exitCode":0}"#),
    ] {
        let value = parse(&envelope);
        let live = one_view(view_from_envelope(&value, SESSION));
        let carried = match &live {
            SessionEvent::AgentToolCall {
                command, exit_code, ..
            }
            | SessionEvent::AgentToolUpdate {
                command, exit_code, ..
            } => (command.clone(), *exit_code),
            other => panic!("expected a tool row, got {other:?}"),
        };
        assert!(
            carried.0.is_some() || carried.1.is_some(),
            "the live row must carry a field or this comparison is vacuous: {envelope}"
        );

        let mut replay_line = value;
        let replayed = one_view(drive_replay(&mut ClaudeView::new(None), &mut replay_line));
        assert_eq!(replayed, live, "envelope: {envelope}");
    }
}
