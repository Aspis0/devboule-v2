//! The resume wire: the tagged outcome, the absent-when-unnegotiated reply,
//! and the frame budget the tail is measured against.
//!
//! The budget cases build the whole `SessionAttached` reply and measure the
//! serialized frame, because a tail that fits 256 KiB of events can still push
//! the frame past `MAX_FRAME_BYTES` once JSON escaping is paid.

use serde_json::json;

use crate::{
    Cursor, DaemonMessage, SessionResumeInfo, SessionResumeOutcome, SessionResumeReason,
    SessionResumeTail, MAX_FRAME_BYTES,
};

fn tail(events: Vec<crate::SessionEvent>, cursor: Cursor) -> SessionResumeTail {
    SessionResumeTail {
        cursor,
        events,
        tail_complete: false,
    }
}

fn message(resume: Option<SessionResumeInfo>) -> DaemonMessage {
    DaemonMessage::SessionAttached {
        id: 7,
        subscription_id: 3,
        resume,
    }
}

/// A text whose every byte forces the longest escape JSON has, so the frame
/// measurement pays the worst case rather than a friendly average.
fn hostile_text(seed: usize, len: usize) -> String {
    const HOSTILE: [&str; 6] = ["\u{0000}", "\n", "\r", "\t", "\"", "\\"];
    let mut out = String::with_capacity(len * 6);
    let mut index = seed;
    while out.len() < len {
        out.push_str(HOSTILE[index % HOSTILE.len()]);
        index += 1;
    }
    out
}

/// The largest tail the daemon's own budget admits: as many hostile events as
/// fit in 256 KiB of wire bytes.
fn biggest_tail() -> SessionResumeTail {
    let budget = 256 * 1024;
    let mut events = Vec::new();
    let mut bytes = 0usize;
    for index in 0.. {
        let event = crate::SessionEvent::AgentMessage {
            message_id: Some(format!("m{index}")),
            text: hostile_text(index, 2048),
            parent_tool_use_id: None,
            spawn_depth: None,

            images: Vec::new(),
        };
        bytes += serde_json::to_vec(&event).expect("event bytes").len();
        if bytes > budget {
            break;
        }
        events.push(event);
    }
    assert!(!events.is_empty(), "the budget must admit something");
    tail(
        events,
        Cursor {
            generation: 4,
            seq: 1_000,
        },
    )
}

#[test]
fn a_reset_is_tagged_and_its_reason_is_snake_case() {
    let info = SessionResumeInfo {
        resume: SessionResumeOutcome::Reset {
            reason: SessionResumeReason::CursorCompacted,
            tail: tail(
                vec![crate::SessionEvent::AgentMessage {
                    message_id: None,
                    text: "newest".into(),
                    parent_tool_use_id: None,
                    spawn_depth: None,

                    images: Vec::new(),
                }],
                Cursor {
                    generation: 4,
                    seq: 9,
                },
            ),
        },
        oldest_seq: 3,
        head: 9,
    };
    let value = serde_json::to_value(message(Some(info))).expect("json");
    assert_eq!(value["resume"]["outcome"], json!("reset"));
    assert_eq!(value["resume"]["reason"], json!("cursor_compacted"));
    assert_eq!(value["resume"]["tail"]["cursor"]["seq"], json!(9));
    assert_eq!(value["resume"]["oldest_seq"], json!(3));
    assert_eq!(value["resume"]["head"], json!(9));
    assert_eq!(
        value["resume"]["tail"]["events"][0]["text"],
        json!("newest")
    );
}

/// A connection that did not negotiate the capability must read the reply it
/// has always read: same keys, byte for byte, and nothing where the resume
/// fields would be.
#[test]
fn an_absent_resume_field_serializes_to_nothing() {
    let value = serde_json::to_value(message(None)).expect("json");
    assert_eq!(
        value,
        json!({ "type": "session_attached", "id": 7, "subscriptionId": 3 }),
        "the unnegotiated reply must stay byte-identical"
    );
    let bytes = serde_json::to_vec(&message(None)).expect("bytes");
    assert_eq!(
        bytes,
        br#"{"type":"session_attached","id":7,"subscriptionId":3}"#
    );
}

/// A resumed outcome carries no tail, so the two cases never overlap on the
/// wire and a client can switch on one field.
#[test]
fn a_resume_is_tagged_and_carries_no_tail() {
    let value = serde_json::to_value(message(Some(SessionResumeInfo {
        resume: SessionResumeOutcome::Resumed,
        oldest_seq: 1,
        head: 12,
    })))
    .expect("json");
    assert_eq!(value["resume"]["outcome"], json!("resumed"));
    assert!(value["resume"]["tail"].is_null());
    assert!(value["resume"]["reason"].is_null());
}

/// The daemon holds the tail to 256 KiB of wire bytes; the frame around it must
/// still stay inside the cap it is sent through.
#[test]
fn the_largest_tail_with_hostile_escaping_stays_under_the_frame_cap() {
    let frame = serde_json::to_vec(&message(Some(SessionResumeInfo {
        resume: SessionResumeOutcome::Reset {
            reason: SessionResumeReason::JournalGap,
            tail: biggest_tail(),
        },
        oldest_seq: 1,
        head: 1_000,
    })))
    .expect("bytes");
    assert!(
        frame.len() < MAX_FRAME_BYTES,
        "the whole reply must stay under MAX_FRAME_BYTES, got {} bytes",
        frame.len()
    );
}

/// The reason the tail's budget is paid on wire bytes and not on stored text:
/// a payload's compact serialized form is already far larger than the string it
/// carries, and the frame pays for the JSON nesting on top of that.
#[test]
fn escaping_expands_a_payload_far_past_its_stored_length() {
    let text = hostile_text(0, 2048);
    let stored = serde_json::to_vec(&text).expect("stored");
    assert!(
        stored.len() > text.len() * 2,
        "the fixture must actually pay the escaping it claims: text {} stored {}",
        text.len(),
        stored.len()
    );
    let framed = serde_json::to_vec(&message(Some(SessionResumeInfo {
        resume: SessionResumeOutcome::Reset {
            reason: SessionResumeReason::CursorAhead,
            tail: tail(
                vec![crate::SessionEvent::AgentMessage {
                    message_id: Some("m0".into()),
                    text,
                    parent_tool_use_id: None,
                    spawn_depth: None,

                    images: Vec::new(),
                }],
                Cursor {
                    generation: 1,
                    seq: 1,
                },
            ),
        },
        oldest_seq: 0,
        head: 1,
    })))
    .expect("bytes");
    assert!(
        framed.len() > stored.len(),
        "the frame wraps the stored payload again: stored {} framed {}",
        stored.len(),
        framed.len()
    );
}

/// A round trip through the wire's own types, so a rename on either side of the
/// tag fails here rather than in an app.
#[test]
fn every_reason_round_trips_under_its_wire_name() {
    for (reason, name) in [
        (SessionResumeReason::EpochChanged, "epoch_changed"),
        (SessionResumeReason::CursorAhead, "cursor_ahead"),
        (SessionResumeReason::CursorCompacted, "cursor_compacted"),
        (SessionResumeReason::JournalGap, "journal_gap"),
    ] {
        let info = SessionResumeInfo {
            resume: SessionResumeOutcome::Reset {
                reason,
                tail: tail(
                    Vec::new(),
                    Cursor {
                        generation: 2,
                        seq: 5,
                    },
                ),
            },
            oldest_seq: 2,
            head: 5,
        };
        let text = serde_json::to_string(&message(Some(info))).expect("json");
        assert!(text.contains(&format!("\"reason\":\"{name}\"")), "{text}");
        let back: DaemonMessage = serde_json::from_str(&text).expect("decode");
        let DaemonMessage::SessionAttached { resume, .. } = back else {
            panic!("attach reply must decode as SessionAttached");
        };
        assert_eq!(
            resume.expect("resume present"),
            SessionResumeInfo {
                resume: SessionResumeOutcome::Reset {
                    reason,
                    tail: tail(
                        Vec::new(),
                        Cursor {
                            generation: 2,
                            seq: 5,
                        },
                    ),
                },
                oldest_seq: 2,
                head: 5,
            }
        );
    }
}

/// The reset the app reads key for key. The frontend mirrors these names in
/// `src/types/ipc.ts`, and nothing in that file is compiled against this crate,
/// so a rename on either side has to fail here rather than in a browser.
#[test]
fn a_reset_carries_exactly_the_keys_the_app_reads() {
    let value = serde_json::to_value(message(Some(SessionResumeInfo {
        resume: SessionResumeOutcome::Reset {
            reason: SessionResumeReason::JournalGap,
            tail: SessionResumeTail {
                cursor: Cursor {
                    generation: 4,
                    seq: 9,
                },
                events: vec![crate::SessionEvent::AgentMessage {
                    message_id: Some("m-9".to_string()),
                    text: "tail row".to_string(),
                    parent_tool_use_id: None,
                    spawn_depth: None,

                    images: Vec::new(),
                }],
                tail_complete: false,
            },
        },
        oldest_seq: 3,
        head: 9,
    })))
    .expect("json");
    assert_eq!(
        value["resume"],
        json!({
            "outcome": "reset",
            "reason": "journal_gap",
            "tail": {
                "cursor": { "generation": 4, "seq": 9 },
                "events": [{
                    "type": "agent_message",
                    "messageId": "m-9",
                    "text": "tail row",
                }],
                "tail_complete": false,
            },
            "oldest_seq": 3,
            "head": 9,
        }),
        "the app replaces its timeline from exactly these keys"
    );
}
