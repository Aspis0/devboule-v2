//! Tests for one topic: the transcript notices the view derives from frames
//! that are not messages — compaction boundaries and local slash-command
//! output. The view's other suites live beside their subjects in the
//! `claude_view_*.rs` files.

use devboule_protocol::{NoticeSeverity, SessionEvent};
use serde_json::json;

use super::ClaudeView;

fn view() -> ClaudeView {
    ClaudeView::new(None)
}

fn notices(events: &[SessionEvent]) -> Vec<(String, NoticeSeverity)> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::SessionNotice { text, severity } => Some((text.clone(), *severity)),
            _ => None,
        })
        .collect()
}

/// The CLI's `compact_boundary` system frame. Documented stream-json shape
/// (no live capture exists in this tree): the trigger field decides the
/// sentence, `pre_tokens` is informational. The metadata key has three
/// spellings in the wild; `key` pins which one this frame uses.
fn compact_boundary_in(key: &str, trigger: &str) -> serde_json::Value {
    json!({
        "type": "system",
        "subtype": "compact_boundary",
        "session_id": "peer-1",
        key: {"trigger": trigger, "pre_tokens": 52345},
    })
}

fn compact_boundary(trigger: &str) -> serde_json::Value {
    compact_boundary_in("compact_metadata", trigger)
}

#[test]
fn back_to_back_compaction_boundaries_announce_one_compaction() {
    let mut view = view();
    let first = view.ingest(&compact_boundary("manual"));
    assert_eq!(
        notices(&first),
        vec![(
            "Context manually compacted".to_string(),
            NoticeSeverity::Info
        )],
        "the boundary announces the compaction"
    );
    let repeat = view.ingest(&compact_boundary("manual"));
    assert!(
        notices(&repeat).is_empty(),
        "a back-to-back repeat is the same compaction: {repeat:?}"
    );
    // Unrelated frames inside the compaction change nothing: the summary
    // text and status lines a compaction produces sit between two boundary
    // heartbeats of the one compaction.
    let summary = view.ingest(&json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "after the compaction"}],
        },
    }));
    assert!(notices(&summary).is_empty());
    let status = view.ingest(&json!({
        "type": "system",
        "subtype": "status",
    }));
    assert!(notices(&status).is_empty());
    let heartbeat = view.ingest(&compact_boundary("manual"));
    assert!(
        notices(&heartbeat).is_empty(),
        "frames between two boundaries do not re-arm: {heartbeat:?}"
    );
    // Only a turn end re-arms: the next compaction's boundary announces.
    // The result row itself carries no notice; ingesting it is what flips
    // the latch.
    let result = view.ingest(&json!({
        "type": "result",
        "subtype": "success",
        "stop_reason": "end_turn",
    }));
    assert!(notices(&result).is_empty());
    let rearmed = view.ingest(&compact_boundary("manual"));
    assert_eq!(
        notices(&rearmed),
        vec![(
            "Context manually compacted".to_string(),
            NoticeSeverity::Info
        )],
        "a turn end re-arms the marker for the next compaction"
    );
}

#[test]
fn an_automatic_compaction_boundary_names_the_trigger() {
    let mut view = view();
    assert_eq!(
        notices(&view.ingest(&compact_boundary("auto"))),
        vec![(
            "Context automatically compacted".to_string(),
            NoticeSeverity::Info
        )],
    );
}

/// The wrapper the CLI wraps local slash-command output in, inside the user
/// envelope it emits for the command's turn.
#[test]
fn a_local_command_stdout_wrapper_publishes_its_inner_text() {
    let mut view = view();
    let events = view.ingest(&json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{
                "type": "text",
                "text": "<local-command-stdout>\nCurrent usage: 12% of your weekly budget.\n</local-command-stdout>",
            }],
        },
    }));
    assert_eq!(
        notices(&events),
        vec![(
            "Current usage: 12% of your weekly budget.".to_string(),
            NoticeSeverity::Info
        )],
        "the inner text is the notice, with no raw tags: {events:?}"
    );
}

#[test]
fn an_empty_local_command_stdout_publishes_nothing() {
    let mut view = view();
    let events = view.ingest(&json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{
                "type": "text",
                "text": "<local-command-stdout>\n</local-command-stdout>",
            }],
        },
    }));
    assert!(notices(&events).is_empty(), "{events:?}");
}

#[test]
fn a_tool_result_beside_the_wrapper_still_maps() {
    let mut view = view();
    let events = view.ingest(&json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "tool-1",
                    "content": "the tool's own output",
                },
                {
                    "type": "text",
                    "text": "<local-command-stdout>slash answer</local-command-stdout>",
                },
            ],
        },
    }));
    assert_eq!(
        notices(&events),
        vec![("slash answer".to_string(), NoticeSeverity::Info)],
        "the wrapper is a notice: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentToolUpdate { text, .. }
                if text.as_deref() == Some("the tool's own output")
        )),
        "the tool_result block beside it maps as before: {events:?}"
    );
}

#[test]
fn a_camel_compact_metadata_boundary_says_manual() {
    let mut view = view();
    assert_eq!(
        notices(&view.ingest(&compact_boundary_in("compactMetadata", "manual"))),
        vec![(
            "Context manually compacted".to_string(),
            NoticeSeverity::Info
        )],
        "the camel spelling announces a manual compaction as manual"
    );
}

#[test]
fn a_compaction_metadata_boundary_says_manual() {
    let mut view = view();
    assert_eq!(
        notices(&view.ingest(&compact_boundary_in("compactionMetadata", "manual"))),
        vec![(
            "Context manually compacted".to_string(),
            NoticeSeverity::Info
        )],
        "the long spelling announces a manual compaction as manual"
    );
}

#[test]
fn a_string_content_wrapper_publishes_its_inner_text() {
    let mut view = view();
    let events = view.ingest(&json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": "<local-command-stdout>usage answer</local-command-stdout>",
        },
    }));
    assert_eq!(
        notices(&events),
        vec![("usage answer".to_string(), NoticeSeverity::Info)],
        "the string shape is the same answer: {events:?}"
    );
}

#[test]
fn a_plain_string_content_publishes_no_notice() {
    let mut view = view();
    let events = view.ingest(&json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": "just a chat line",
        },
    }));
    assert!(notices(&events).is_empty(), "{events:?}");
}
