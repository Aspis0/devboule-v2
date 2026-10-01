//! Tests for one topic: stream tracking — deltas matched to the
//! consolidated envelope blocks they belong to, across parent, child and
//! sibling streams.

use devboule_protocol::SessionEvent;
use serde_json::{json, Value};

use crate::claude_view::test_support::view;

#[test]
fn stream_event_text_delta_becomes_agent_message() {
    // recon/probes/claude-streaming-shape.txt
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "1"}
        },
        "session_id": "3989672c-b6c4-4f26-886c-fb8dc3446418"
    }));
    assert_eq!(
        events,
        vec![SessionEvent::AgentMessage {
            message_id: None,
            text: "1".to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
        }]
    );
}

#[test]
fn stream_event_thinking_delta_becomes_agent_thought() {
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "Let me think."}
        }
    }));
    assert_eq!(
        events,
        vec![SessionEvent::AgentThought {
            message_id: None,
            text: "Let me think.".to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
        }]
    );
}

#[test]
fn consolidated_assistant_after_deltas_emits_only_the_remainder() {
    // recon/probes/claude-streaming-shape.txt: deltas then a full assistant.
    let mut mapper = view();
    let _ = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "message_start", "message": {"id": "msg_011CeiQ5WgY6ewJ4proDpmny", "model": "claude-opus-5"}}
    }));
    let _ = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "1\n2\n3\n4\n5"}}
    }));
    let _ = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "\n6\n7\n8\n9"}}
    }));
    let remainder = mapper.ingest(&json!({
        "type": "assistant",
        "message": {
            "model": "claude-opus-5",
            "id": "msg_011CeiQ5WgY6ewJ4proDpmny",
            "role": "assistant",
            "content": [{"type": "text", "text": "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15"}]
        }
    }));
    assert_eq!(
        remainder,
        vec![SessionEvent::AgentMessage {
            message_id: Some("msg_011CeiQ5WgY6ewJ4proDpmny".to_string()),
            text: "\n10\n11\n12\n13\n14\n15".to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
        }]
    );
}

/// Feed envelopes in order, collecting the assistant text and thought
/// fragments the view emits.
fn fed_texts(envelopes: &[Value]) -> (Vec<String>, Vec<String>) {
    let mut mapper = view();
    let mut texts = Vec::new();
    let mut thoughts = Vec::new();
    for envelope in envelopes {
        for event in mapper.ingest(envelope) {
            match event {
                SessionEvent::AgentMessage { text, .. } => texts.push(text),
                SessionEvent::AgentThought { text, .. } => thoughts.push(text),
                _ => {}
            }
        }
    }
    (texts, thoughts)
}

// Shape of reports/remote-agents/EVIDENCE-claude-double-text.jsonl
// (field test, 2026-09-16): Claude emits one `assistant` envelope per
// content block, each carrying a single-block `content` array, so every
// block after the first sits at array position 0 while its stream index
// is higher. The signature is abbreviated; the sequence is verbatim.
fn thinking_then_answer_turn() -> Vec<Value> {
    vec![
        json!({"type": "stream_event", "event": {"type": "message_start", "message": {"id": "msg_double", "model": "claude-haiku-4-5"}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "", "estimated_tokens": 50}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}}}),
        json!({"type": "assistant", "message": {"model": "claude-haiku-4-5", "id": "msg_double", "role": "assistant", "content": [{"type": "thinking", "thinking": "", "signature": "sig"}]}}),
        json!({"type": "stream_event", "event": {"type": "content_block_stop", "index": 0}}),
        json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "CHILD"}}}),
        json!({"type": "assistant", "message": {"model": "claude-haiku-4-5", "id": "msg_double", "role": "assistant", "content": [{"type": "text", "text": "CHILD"}]}}),
        json!({"type": "stream_event", "event": {"type": "content_block_stop", "index": 1}}),
    ]
}

#[test]
fn one_assistant_envelope_per_block_after_thinking_emits_the_answer_once() {
    let (texts, thoughts) = fed_texts(&thinking_then_answer_turn());
    assert_eq!(texts, ["CHILD"], "the answer must be emitted exactly once");
    assert!(
        thoughts.is_empty(),
        "empty thinking must not become a thought"
    );
}

#[test]
fn single_text_block_streamed_then_confirmed_emits_once() {
    // The pre-existing shape: one text block at stream index 0, streamed,
    // then a final envelope whose only block repeats the same text.
    let (texts, thoughts) = fed_texts(&[
        json!({"type": "stream_event", "event": {"type": "message_start", "message": {"id": "msg_pong", "model": "claude-opus-5"}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "PONG"}}}),
        json!({"type": "assistant", "message": {"model": "claude-opus-5", "id": "msg_pong", "role": "assistant", "content": [{"type": "text", "text": "PONG"}]}}),
    ]);
    assert_eq!(texts, ["PONG"]);
    assert!(thoughts.is_empty());
}

#[test]
fn one_assistant_envelope_carrying_all_blocks_emits_the_answer_once() {
    // Both blocks streamed, then a single final envelope whose
    // `content` array carries them together.
    let (texts, thoughts) = fed_texts(&[
        json!({"type": "stream_event", "event": {"type": "message_start", "message": {"id": "msg_all", "model": "claude-haiku-4-5"}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "hmm", "estimated_tokens": 1}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "CHILD"}}}),
        json!({"type": "assistant", "message": {"model": "claude-haiku-4-5", "id": "msg_all", "role": "assistant", "content": [
            {"type": "thinking", "thinking": "hmm", "signature": "sig"},
            {"type": "text", "text": "CHILD"}
        ]}}),
    ]);
    assert_eq!(texts, ["CHILD"]);
    assert_eq!(thoughts, ["hmm"]);
}

#[test]
fn two_same_type_blocks_in_separate_envelopes_emit_both_whole() {
    // The Anthropic API permits more than one text block per message. When
    // the producer delivers them in separate envelopes, ordinal matching
    // would claim the lowest stream index twice and slice the second text.
    let (texts, thoughts) = fed_texts(&[
        json!({"type": "stream_event", "event": {"type": "message_start", "message": {"id": "msg_two_texts", "model": "claude-opus-5"}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "CHILD"}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 3, "content_block": {"type": "text", "text": ""}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 3, "delta": {"type": "text_delta", "text": "SECOND"}}}),
        json!({"type": "assistant", "message": {"model": "claude-opus-5", "id": "msg_two_texts", "role": "assistant", "content": [{"type": "text", "text": "CHILD"}]}}),
        json!({"type": "assistant", "message": {"model": "claude-opus-5", "id": "msg_two_texts", "role": "assistant", "content": [{"type": "text", "text": "SECOND"}]}}),
    ]);
    assert_eq!(texts, ["CHILD", "SECOND"]);
    assert!(thoughts.is_empty());
}

#[test]
fn a_redelivered_identical_block_text_is_emitted_again_rather_than_lost() {
    // One stream block satisfies at most one envelope
    // block: a matcher that let one stream block confirm every envelope
    // block that asked kept this sequence silent — and also
    // swallowed a genuine second block with the same text
    // (`two_identical_blocks_in_one_envelope_emit_both`). The two shapes
    // are the same bytes to a text matcher: the envelope block either
    // emits or is dropped, and the dropped side is the one nothing
    // notices. This envelope therefore emits again — visible duplication beats
    // silent loss.
    let (texts, thoughts) = fed_texts(&[
        json!({"type": "stream_event", "event": {"type": "message_start", "message": {"id": "msg_redelivery", "model": "claude-opus-5"}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "CHILD"}}}),
        json!({"type": "assistant", "message": {"model": "claude-opus-5", "id": "msg_redelivery", "role": "assistant", "content": [{"type": "text", "text": "CHILD"}]}}),
        json!({"type": "assistant", "message": {"model": "claude-opus-5", "id": "msg_redelivery", "role": "assistant", "content": [{"type": "text", "text": "CHILD"}]}}),
    ]);
    assert_eq!(texts, ["CHILD", "CHILD"]);
    assert!(thoughts.is_empty());
}

#[test]
fn two_identical_blocks_in_one_envelope_emit_both() {
    // The matcher picks a stream block by declared kind and streamed
    // prefix alone, so two envelope blocks carrying the same text
    // collapsed onto the one streamed block and the second was silently
    // dropped: the raw envelope in the journal kept it, the derived view lost it.
    let (texts, thoughts) = fed_texts(&[
        json!({"type": "stream_event", "event": {"type": "message_start", "message": {"id": "msg_twin", "model": "claude-opus-5"}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}}),
        json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "X"}}}),
        json!({"type": "assistant", "message": {"model": "claude-opus-5", "id": "msg_twin", "role": "assistant", "content": [
            {"type": "text", "text": "X"},
            {"type": "text", "text": "X"}
        ]}}),
    ]);
    assert_eq!(
        texts,
        ["X", "X"],
        "the second block's text must be emitted, not collapsed onto the first"
    );
    assert!(thoughts.is_empty());
}

#[test]
fn a_confirmed_block_shorter_or_unequal_is_not_re_emitted() {
    // A provider that normalises text on confirmation (a quote, a dash,
    // an entity) confirms with a text that is no longer a prefix of the
    // stream's. The deltas already showed at least that much, so the
    // envelope confirms the block; the whole text must not ride on top
    // of what the deltas emitted.
    let confirmed = |text: &str| {
        let (texts, _) = fed_texts(&[
            json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello!"}}}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}),
        ]);
        texts
    };
    assert_eq!(
        confirmed("Hello?"),
        ["Hello!"],
        "an equal-length rewrite confirms instead of repeating"
    );
    assert_eq!(
        confirmed("Hi"),
        ["Hello!"],
        "a shorter envelope confirms instead of repeating"
    );
}

#[test]
fn content_block_stop_drops_the_block_text() {
    // A stream whose message id never arrives — `note_message` returns
    // before either retain runs — would hold every block's full
    // accumulated text for the daemon's lifetime. The stop each block
    // sends, after its confirmation, is what ends it.
    let mut mapper = view();
    for text in ["first half ", "second half"] {
        let _ = mapper.ingest(&json!({
            "type": "stream_event",
            "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}},
        }));
    }
    let held: Vec<&str> = mapper.streamed.values().map(|b| b.text.as_str()).collect();
    assert_eq!(held, ["first half second half"]);
    let _ = mapper.ingest(&json!({
        "type": "assistant",
        "message": {"content": [{"type": "text", "text": "first half second half"}]}
    }));
    let _ = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "content_block_stop", "index": 0},
    }));
    let held: Vec<&str> = mapper.streamed.values().map(|b| b.text.as_str()).collect();
    assert!(
        held.is_empty(),
        "the block's text outlives its stop: {held:?}"
    );
}

#[test]
fn interleaved_parent_and_child_streams_keep_text_and_remainders_separate() {
    let mut mapper = view();
    let _ = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "message_start", "message": {"id": "msg_parent", "model": "claude-opus-5"}},
        "parent_tool_use_id": null
    }));
    let parent_first = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "parent-1"}},
        "parent_tool_use_id": null
    }));
    let _ = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "message_start", "message": {"id": "msg_child", "model": "claude-opus-5"}},
        "parent_tool_use_id": "toolu_agent",
        "spawn_depth": 1
    }));
    let child = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "child"}},
        "parent_tool_use_id": "toolu_agent",
        "spawn_depth": 1
    }));
    let parent_second = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "parent-2"}},
        "parent_tool_use_id": null
    }));

    assert!(matches!(
        parent_first.as_slice(),
        [SessionEvent::AgentMessage { message_id: Some(id), text, parent_tool_use_id: None, .. }]
            if id == "msg_parent" && text == "parent-1"
    ));
    assert!(matches!(
        child.as_slice(),
        [SessionEvent::AgentMessage { message_id: Some(id), text, parent_tool_use_id: Some(parent), .. }]
            if id == "msg_child" && text == "child" && parent == "toolu_agent"
    ));
    assert!(matches!(
        parent_second.as_slice(),
        [SessionEvent::AgentMessage { message_id: Some(id), text, parent_tool_use_id: None, .. }]
            if id == "msg_parent" && text == "parent-2"
    ));

    let parent_remainder = mapper.ingest(&json!({
        "type": "assistant",
        "message": {"id": "msg_parent", "content": [{"type": "text", "text": "parent-1parent-2"}]},
        "parent_tool_use_id": null
    }));
    let child_remainder = mapper.ingest(&json!({
        "type": "assistant",
        "message": {"id": "msg_child", "content": [{"type": "text", "text": "child"}]},
        "parent_tool_use_id": "toolu_agent",
        "spawn_depth": 1
    }));
    assert!(parent_remainder.is_empty());
    assert!(child_remainder.is_empty());
}

#[test]
fn sibling_streams_with_same_block_index_keep_text_separate() {
    let mut mapper = view();
    for (message_id, parent_tool_use_id) in
        [("msg_sibling_a", "toolu_a"), ("msg_sibling_b", "toolu_b")]
    {
        let _ = mapper.ingest(&json!({
            "type": "stream_event",
            "event": {"type": "message_start", "message": {"id": message_id}},
            "parent_tool_use_id": parent_tool_use_id,
            "spawn_depth": 1
        }));
    }
    let first = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "sibling-a"}},
        "parent_tool_use_id": "toolu_a",
        "spawn_depth": 1
    }));
    let second = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "sibling-b"}},
        "parent_tool_use_id": "toolu_b",
        "spawn_depth": 1
    }));

    assert!(matches!(
        first.as_slice(),
        [SessionEvent::AgentMessage {
            text,
            parent_tool_use_id: Some(parent),
            ..
        }] if text == "sibling-a" && parent == "toolu_a"
    ));
    assert!(matches!(
        second.as_slice(),
        [SessionEvent::AgentMessage {
            text,
            parent_tool_use_id: Some(parent),
            ..
        }] if text == "sibling-b" && parent == "toolu_b"
    ));

    for (message_id, parent_tool_use_id, text) in [
        ("msg_sibling_a", "toolu_a", "sibling-a"),
        ("msg_sibling_b", "toolu_b", "sibling-b"),
    ] {
        let remainder = mapper.ingest(&json!({
            "type": "assistant",
            "message": {"id": message_id, "content": [{"type": "text", "text": text}]},
            "parent_tool_use_id": parent_tool_use_id,
            "spawn_depth": 1
        }));
        assert!(
            remainder.is_empty(),
            "unexpected remainder for {parent_tool_use_id}"
        );
    }
}

#[test]
fn empty_and_null_parent_tool_use_ids_share_the_root_stream() {
    let mut mapper = view();
    let _ = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "message_start", "message": {"id": "msg_root"}},
        "parent_tool_use_id": null
    }));
    let _ = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "root-"}},
        "parent_tool_use_id": null
    }));
    let empty_parent = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "text"}},
        "parent_tool_use_id": ""
    }));

    assert!(matches!(
        empty_parent.as_slice(),
        [SessionEvent::AgentMessage {
            message_id: Some(message_id),
            parent_tool_use_id: None,
            text,
            ..
        }] if message_id == "msg_root" && text == "text"
    ));
    let remainder = mapper.ingest(&json!({
        "type": "assistant",
        "message": {"id": "msg_root", "content": [{"type": "text", "text": "root-text"}]},
        "parent_tool_use_id": null
    }));
    assert!(remainder.is_empty());
}
