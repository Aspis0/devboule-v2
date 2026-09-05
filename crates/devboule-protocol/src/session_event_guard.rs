//! Committed snapshot of every [`crate::SessionEvent`] wire payload.
//!
//! The daemon decides what it sends. This module is the mechanical link to
//! the TypeScript handler: the sample list is built from an exhaustive match
//! on `SessionEvent` (a new variant that is not constructed here is a compile
//! error), the committed JSON is regenerated and compared (a stale file is a
//! test failure), and the TypeScript `SessionEvent` union is parsed for the
//! same `type` tags.
//!
//! The frontend test in `src/features/terminal/terminalSession.test.ts` then
//! feeds each committed sample to the handler and requires that it does not
//! land in the unknown-event error branch.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{
    AgentActivityState, CursorShape, ScreenCursor, SessionEvent, TranscriptIntegrity,
};

/// One dummy of every [`SessionEvent`] variant.
///
/// The `match` and the `vec` are expanded from the same list. Adding a
/// variant to the enum without a dummy here is a compile error. Wire `type`
/// tags are taken from serde, not from these identifiers.
fn session_event_samples() -> Vec<SessionEvent> {
    macro_rules! samples {
        ($($variant:ident => $sample:expr),+ $(,)?) => {{
            let events: Vec<SessionEvent> = vec![$($sample),+];
            for event in &events {
                match event {
                    $(SessionEvent::$variant { .. } => {})+
                }
            }
            events
        }};
    }

    samples!(
        Output => SessionEvent::Output {
            seq: 0,
            data: String::new(),
        },
        AgentMessage => SessionEvent::AgentMessage {
            message_id: None,
            text: String::new(),
        },
        AgentUserMessage => SessionEvent::AgentUserMessage {
            message_id: None,
            text: String::new(),
        },
        AgentThought => SessionEvent::AgentThought {
            message_id: None,
            text: String::new(),
        },
        AvailableCommands => SessionEvent::AvailableCommands {
            commands: Vec::new(),
        },
        AgentToolCall => SessionEvent::AgentToolCall {
            tool_call_id: String::new(),
            title: String::new(),
            status: String::new(),
        },
        AgentToolUpdate => SessionEvent::AgentToolUpdate {
            tool_call_id: String::new(),
            status: None,
            text: None,
        },
        AgentFinished => SessionEvent::AgentFinished {
            stop_reason: String::new(),
            model_id: None,
            usage: None,
        },
        AgentError => SessionEvent::AgentError {
            message: String::new(),
        },
        AgentStderr => SessionEvent::AgentStderr {
            data: String::new(),
        },
        PermissionRequest => SessionEvent::PermissionRequest {
            tool_call_id: String::new(),
            title: String::new(),
            description: None,
            command: None,
            args: Some(Vec::new()),
            cwd: None,
            env: Some(Vec::new()),
            options: Vec::new(),
        },
        PermissionResolved => SessionEvent::PermissionResolved {
            tool_call_id: String::new(),
        },
        SessionManifest => SessionEvent::SessionManifest {
            provider_id: None,
            current_model_id: None,
            models: Vec::new(),
            modes: None,
        },
        AgentReported => SessionEvent::AgentReported {
            seq: 0,
            source: String::new(),
            agent: String::new(),
            state: AgentActivityState::Idle,
            message: None,
            report_seq: None,
            agent_session_id: None,
            agent_session_path: None,
            session_start_source: None,
        },
        Exit => SessionEvent::Exit { code: None },
        Silent => SessionEvent::Silent { elapsed_ms: 0 },
        Recovered => SessionEvent::Recovered {
            integrity: TranscriptIntegrity::Unverifiable {
                dropped_frames: 0,
                dropped_bytes: 0,
                trimmed_bytes: 0,
            },
        },
        JournalDegraded => SessionEvent::JournalDegraded {
            dropped_frames: 0,
            dropped_bytes: 0,
        },
        SessionsSnapshot => SessionEvent::SessionsSnapshot {
            sessions: Vec::new(),
        },
        Snapshot => SessionEvent::Snapshot {
            as_of_seq: 0,
            cols: 0,
            rows: 0,
            data: String::new(),
            cursor: ScreenCursor {
                row: 0,
                col: 0,
                visible: false,
                shape: CursorShape::Block,
                blinking: false,
            },
            alternate_screen: false,
            bracketed_paste: false,
            line_wrap: false,
            title: None,
        },
    )
}

fn session_event_snapshot_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("session-event-samples.generated.json")
}

fn frontend_ipc_ts_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("src");
    path.push("types");
    path.push("ipc.ts");
    path
}

fn render_session_event_snapshot(events: &[SessionEvent]) -> String {
    let mut samples: Vec<Value> = events
        .iter()
        .map(|event| serde_json::to_value(event).expect("SessionEvent JSON"))
        .collect();
    samples.sort_by(|left, right| {
        wire_type(left)
            .expect("sample type")
            .cmp(&wire_type(right).expect("sample type"))
    });
    let mut text = serde_json::to_string_pretty(&Value::Array(samples)).expect("pretty JSON");
    text.push('\n');
    text
}

fn wire_type(value: &Value) -> Option<String> {
    value.get("type")?.as_str().map(str::to_owned)
}

fn session_event_wire_types(events: &[SessionEvent]) -> BTreeSet<String> {
    let mut types = BTreeSet::new();
    for event in events {
        let value = serde_json::to_value(event).expect("SessionEvent JSON");
        let Some(tag) = wire_type(&value) else {
            panic!("{event:?} serialized without a type tag: {value}");
        };
        if !types.insert(tag.clone()) {
            panic!("duplicate SessionEvent sample for type {tag}");
        }
    }
    types
}

fn read_text_lf(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .replace("\r\n", "\n")
}

fn strip_ts_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            continue;
        }
        if c == '/' {
            match chars.peek() {
                Some('/') => {
                    chars.next();
                    for ch in chars.by_ref() {
                        if ch == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    let mut prev = '\0';
                    for ch in chars.by_ref() {
                        if prev == '*' && ch == '/' {
                            break;
                        }
                        prev = ch;
                    }
                    out.push(' ');
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

fn type_alias_body<'a>(source: &'a str, marker: &str) -> &'a str {
    let Some(marker_at) = source.find(marker) else {
        panic!("src/types/ipc.ts has no `{marker}`");
    };
    let after_marker = &source[marker_at + marker.len()..];
    let Some(eq_at) = after_marker.find('=') else {
        panic!("`{marker}` has no `=`");
    };
    let after_eq = &after_marker[eq_at + 1..];
    let mut depth = 0i32;
    let mut in_string = false;
    let mut prev = '\0';
    for (index, c) in after_eq.char_indices() {
        if in_string {
            if c == '"' && prev != '\\' {
                in_string = false;
            }
            prev = c;
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => depth -= 1,
            ';' if depth == 0 => return after_eq[..index].trim(),
            _ => {}
        }
        prev = c;
    }
    panic!("`{marker}` has no terminating `;`");
}

fn split_union_members(body: &str) -> Vec<&str> {
    let mut members = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    for (index, c) in body.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            '|' if depth == 0 => {
                let member = body[start..index].trim();
                if !member.is_empty() {
                    members.push(member);
                }
                start = index + c.len_utf8();
            }
            _ => {}
        }
    }
    let member = body[start..].trim();
    if !member.is_empty() {
        members.push(member);
    }
    members
}

fn type_field_literal(member: &str) -> Option<String> {
    const PREFIX: &str = "type:";
    let at = member.find(PREFIX)?;
    let after = member[at + PREFIX.len()..].trim_start();
    let rest = after.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

fn is_ts_type_name(member: &str) -> bool {
    let mut chars = member.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn find_export(source: &str, kind: &str, name: &str) -> Option<usize> {
    let marker = format!("export {kind} {name}");
    let mut search = 0usize;
    while let Some(relative) = source[search..].find(&marker) {
        let absolute = search + relative;
        let after = absolute + marker.len();
        let glued = source[after..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        if !glued {
            return Some(absolute);
        }
        search = after;
    }
    None
}

fn brace_block_from(source: &str) -> &str {
    let Some(open_at) = source.find('{') else {
        panic!("exported TypeScript type has no `{{` body");
    };
    let mut depth = 0i32;
    for (index, c) in source[open_at..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open_at..open_at + index + 1];
                }
            }
            _ => {}
        }
    }
    panic!("exported TypeScript type has an unclosed `{{` body");
}

fn exported_type_block<'a>(source: &'a str, name: &str) -> &'a str {
    if let Some(at) = find_export(source, "interface", name) {
        return brace_block_from(&source[at..]);
    }
    if let Some(at) = find_export(source, "type", name) {
        return type_alias_body(&source[at..], &format!("export type {name}"));
    }
    panic!("src/types/ipc.ts has no exported type `{name}` referenced by SessionEvent");
}

fn session_event_types_in_ipc(source: &str) -> BTreeSet<String> {
    let stripped = strip_ts_comments(source);
    let body = type_alias_body(&stripped, "export type SessionEvent");
    let mut types = BTreeSet::new();
    for member in split_union_members(body) {
        if let Some(tag) = type_field_literal(member) {
            types.insert(tag);
            continue;
        }
        if is_ts_type_name(member) {
            let resolved = exported_type_block(&stripped, member);
            let Some(tag) = type_field_literal(resolved) else {
                panic!(
                    "SessionEvent union member `{member}` has no `type: \"...\"` field in ipc.ts"
                );
            };
            types.insert(tag);
            continue;
        }
        panic!("cannot parse SessionEvent union member `{member}` in src/types/ipc.ts");
    }
    if types.is_empty() {
        panic!("SessionEvent union in src/types/ipc.ts contains no type tags");
    }
    types
}

#[test]
fn session_event_samples_match_committed_snapshot() {
    let events = session_event_samples();
    assert!(
        !events.is_empty(),
        "SessionEvent sample list is empty; the snapshot would pass vacuously"
    );
    let expected = render_session_event_snapshot(&events);
    let path = session_event_snapshot_path();

    if std::env::var_os("UPDATE_SESSION_EVENT_SNAPSHOT").is_some() {
        std::fs::write(&path, &expected).unwrap_or_else(|err| {
            panic!("failed to write {}: {err}", path.display());
        });
    }

    if !path.is_file() {
        panic!(
            "committed SessionEvent snapshot is missing at {}.\n\
             Replace that file with the following contents (or re-run with \
             UPDATE_SESSION_EVENT_SNAPSHOT=1):\n\n{expected}",
            path.display()
        );
    }

    let on_disk = read_text_lf(&path);
    assert_eq!(
        on_disk,
        expected,
        "committed SessionEvent snapshot at {} is stale.\n\
         The daemon's SessionEvent variants no longer match the committed file.\n\
         Replace that file with the expected contents above, or re-run with \
         UPDATE_SESSION_EVENT_SNAPSHOT=1.",
        path.display()
    );
}

#[test]
fn session_event_wire_types_match_frontend_union() {
    let rust_types = session_event_wire_types(&session_event_samples());
    let path = frontend_ipc_ts_path();
    if !path.is_file() {
        panic!(
            "TypeScript SessionEvent union not found at {}. \
             Refusing to skip: this test keeps SessionEvent aligned with src/types/ipc.ts.",
            path.display()
        );
    }
    let source = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", path.display());
    });
    let ts_types = session_event_types_in_ipc(&source);
    assert_eq!(
        rust_types, ts_types,
        "SessionEvent serde type tags and the TypeScript SessionEvent union in src/types/ipc.ts drifted"
    );
}
