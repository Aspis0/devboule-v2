//! Tests for one topic: the file changes pi's edit and write calls carry on
//! their `toolcall_end` arguments, the way Paseo's pi mapper reads them.

use devboule_protocol::{AgentEditKind, AgentEditText, SessionEvent};
use serde_json::{json, Value};

use super::events_from_line;

fn toolcall_end(name: &str, arguments: Value) -> Value {
    json!({
        "type": "message_update",
        "assistantMessageEvent": {
            "type": "toolcall_end",
            "contentIndex": 0,
            "toolCall": {"type": "toolCall", "id": "call_edit", "name": name, "arguments": arguments}
        }
    })
}

fn update_edits(name: &str, arguments: Value) -> Option<Vec<devboule_protocol::AgentToolEdit>> {
    match events_from_line(&toolcall_end(name, arguments)).as_slice() {
        [SessionEvent::AgentToolUpdate { edits, .. }] => edits.clone(),
        other => panic!("expected one tool update, got {other:?}"),
    }
}

#[test]
fn an_edit_call_carries_each_old_and_new_text_of_its_file() {
    let edits = update_edits(
        "edit",
        json!({
            "path": "src/lib.rs",
            "edits": [
                {"oldText": "one\n", "newText": "two\n"},
                {"oldText": "a\n", "newText": "b\nc\n"}
            ]
        }),
    )
    .expect("the edit carries its texts");
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0].path, "src/lib.rs");
    assert_eq!(edits[0].kind, AgentEditKind::Edit);
    assert_eq!(
        edits[0].text,
        Some(AgentEditText::Replace {
            old: "one\n".to_string(),
            new: "two\n".to_string(),
        })
    );
    assert_eq!((edits[1].added, edits[1].removed), (2, 1));
}

#[test]
fn a_write_call_carries_its_whole_content_as_added() {
    let edits = update_edits(
        "write",
        json!({"path": "notes.txt", "content": "hello\nworld\n"}),
    )
    .expect("the write carries its content");
    assert_eq!(edits[0].kind, AgentEditKind::Write);
    assert_eq!((edits[0].added, edits[0].removed), (2, 0));
}

#[test]
fn a_bash_call_carries_no_edits() {
    assert_eq!(update_edits("bash", json!({"command": "ls"})), None);
}
