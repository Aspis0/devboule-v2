//! The per-session queue frames and the queue snapshot event, pinned by
//! shape: wire tags, request ids, audit classification, and the field
//! spellings a client (and Q3's wrappers) are written against.
//!
//! Written before the frames existed, per the Q1 brief's red-first rule.

use super::*;
use crate::session::QueuedMessage;

fn add_frame() -> ClientMessage {
    ClientMessage::SessionQueueAdd {
        id: 7,
        session_id: "s.a.1".to_string(),
        text: "first".to_string(),
        attachments: Vec::new(),
        attachment_references: Vec::new(),
    }
}

#[test]
fn every_queue_frame_answers_its_request_id_and_wire_name() {
    let frames = [
        (add_frame(), "SessionQueueAdd"),
        (
            ClientMessage::SessionQueueEdit {
                id: 8,
                session_id: "s.a.1".to_string(),
                item_id: "queue-1".to_string(),
                text: "edited".to_string(),
            },
            "SessionQueueEdit",
        ),
        (
            ClientMessage::SessionQueueRemove {
                id: 9,
                session_id: "s.a.1".to_string(),
                item_id: "queue-1".to_string(),
            },
            "SessionQueueRemove",
        ),
        (
            ClientMessage::SessionQueueMove {
                id: 10,
                session_id: "s.a.1".to_string(),
                item_id: "queue-1".to_string(),
                to_index: 0,
            },
            "SessionQueueMove",
        ),
        (
            ClientMessage::SessionQueueSendNow {
                id: 11,
                session_id: "s.a.1".to_string(),
                subscription_id: 3,
                item_id: "queue-1".to_string(),
            },
            "SessionQueueSendNow",
        ),
    ];
    for (frame, name) in frames {
        assert_eq!(frame.request_id(), Some(frame.request_id().expect("an id")));
        assert_eq!(frame.name(), name);
        assert!(
            frame.is_state_changing(),
            "{name} changes the queue, so it earns an audit row"
        );
    }
}

#[test]
fn queue_frames_carry_the_wire_tag_the_frontend_will_match() {
    let value = serde_json::to_value(add_frame()).expect("serialize add");
    assert_eq!(value["type"], "session_queue_add");
    assert_eq!(value["sessionId"], "s.a.1");
    // Empty attachment lists are omitted, never sent as `[]` — the same rule
    // `SessionSend` states for its two attachment fields.
    assert!(value.get("attachments").is_none());
    assert!(value.get("attachmentReferences").is_none());

    let value = serde_json::to_value(ClientMessage::SessionQueueMove {
        id: 1,
        session_id: "s.a.1".to_string(),
        item_id: "queue-2".to_string(),
        to_index: 1,
    })
    .expect("serialize move");
    assert_eq!(value["type"], "session_queue_move");
    assert_eq!(value["itemId"], "queue-2");
    assert_eq!(value["toIndex"], 1);
}

#[test]
fn interrupt_is_an_active_turn_behavior_word() {
    let behavior: ActiveTurnBehavior =
        serde_json::from_value(serde_json::Value::String("interrupt".to_string()))
            .expect("interrupt parses");
    assert_eq!(behavior, ActiveTurnBehavior::Interrupt);
    let steer: ActiveTurnBehavior =
        serde_json::from_value(serde_json::Value::String("steer".to_string()))
            .expect("steer parses");
    assert_eq!(steer, ActiveTurnBehavior::Steer);
    assert!(
        serde_json::from_value::<ActiveTurnBehavior>(serde_json::Value::String(
            "queue".to_string()
        ))
        .is_err()
    );
}

#[test]
fn the_queue_snapshot_event_carries_the_whole_queue() {
    let event = SessionEvent::QueueSnapshot {
        items: vec![
            QueuedMessage {
                item_id: "queue-1".to_string(),
                text: "first".to_string(),
                attachment_references: Vec::new(),
                error: None,
            },
            QueuedMessage {
                item_id: "queue-2".to_string(),
                text: "second".to_string(),
                attachment_references: vec![AttachmentReference {
                    session_id: "s.a.1".to_string(),
                    digest: "a".repeat(64),
                    stored_bytes: 128,
                }],
                error: Some("the send failed".to_string()),
            },
        ],
    };
    let value = serde_json::to_value(&event).expect("serialize snapshot");
    assert_eq!(value["type"], "queue_snapshot");
    assert_eq!(value["items"][0]["itemId"], "queue-1");
    assert_eq!(
        value["items"][1]["attachmentReferences"][0]["digest"],
        "a".repeat(64)
    );
    assert_eq!(value["items"][1]["error"], "the send failed");
    // An item with no error omits the key: absent is "no failure", never
    // `null`, never an empty string.
    assert!(value["items"][0].get("error").is_none());
    let back: SessionEvent = serde_json::from_value(value).expect("deserialize snapshot");
    assert_eq!(back, event);
}
