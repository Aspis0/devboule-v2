//! The shared-queue wire: the five request frames, the one snapshot event and
//! the version guard that keeps an older peer off all of them.

use super::*;

/// One sample of every queue request, in wire-name order.
fn queue_requests() -> Vec<ClientMessage> {
    vec![
        ClientMessage::SessionQueueAdd {
            id: 1,
            session_id: "s.a.1".to_string(),
            client_operation_id: "op-1".to_string(),
            text: "first".to_string(),
            attachments: Vec::new(),
            attachment_references: Vec::new(),
        },
        ClientMessage::SessionQueueEdit {
            id: 2,
            session_id: "s.a.1".to_string(),
            client_operation_id: "op-2".to_string(),
            item_id: "queue-1".to_string(),
            text: "rewritten".to_string(),
        },
        ClientMessage::SessionQueueRemove {
            id: 3,
            session_id: "s.a.1".to_string(),
            client_operation_id: "op-3".to_string(),
            item_id: "queue-1".to_string(),
        },
        ClientMessage::SessionQueueMove {
            id: 4,
            session_id: "s.a.1".to_string(),
            client_operation_id: "op-4".to_string(),
            item_id: "queue-1".to_string(),
            to_index: 2,
        },
        ClientMessage::SessionQueueSendNow {
            id: 5,
            session_id: "s.a.1".to_string(),
            client_operation_id: "op-5".to_string(),
            subscription_id: 9,
            item_id: "queue-1".to_string(),
        },
    ]
}

#[test]
fn a_queue_request_round_trips_through_its_wire_shape() {
    for request in queue_requests() {
        let json = serde_json::to_string(&request).expect("serialize");
        let back: ClientMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(request, back, "{json} did not read back as itself");
    }
}

#[test]
fn the_queue_request_tags_are_the_snake_case_variants() {
    // The wire tag is the snake_case spelling of the audit name, the rule
    // every other `ClientMessage` variant follows.
    let expected = [
        ("SessionQueueAdd", "session_queue_add"),
        ("SessionQueueEdit", "session_queue_edit"),
        ("SessionQueueRemove", "session_queue_remove"),
        ("SessionQueueMove", "session_queue_move"),
        ("SessionQueueSendNow", "session_queue_send_now"),
    ];
    assert_eq!(
        queue_requests()
            .iter()
            .map(|r| r.name())
            .collect::<Vec<_>>(),
        expected.iter().map(|(name, _)| *name).collect::<Vec<_>>()
    );
    for (request, (name, tag)) in queue_requests().iter().zip(expected) {
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(request).unwrap()).expect("json");
        assert_eq!(json["type"], tag, "{name} must carry its own wire tag");
        assert!(json.get("id").is_some(), "{name} must carry its own id");
    }
}

#[test]
fn every_queue_request_carries_its_id_and_names_its_session() {
    for request in queue_requests() {
        assert!(
            request.request_id().is_some(),
            "{:?} has no request id",
            request.name()
        );
        assert!(
            request.is_state_changing(),
            "{:?} must be audited",
            request.name()
        );
    }
    // The two attachment lists are optional on an add, so a client that queues
    // text alone omits both keys rather than sending empty arrays.
    let add = serde_json::to_value(&queue_requests()[0]).expect("json");
    assert!(add.get("attachments").is_none());
    assert!(add.get("attachmentReferences").is_none());
}

#[test]
fn every_mutating_queue_request_names_the_operation_it_is() {
    // The operation id is required, not optional: a frame without one cannot
    // be retried safely, and a retry that queues the message twice is the
    // failure this field exists to prevent. So it is a plain required string,
    // unlike `idempotency_key`, which this dialect never offers.
    for request in queue_requests() {
        assert_eq!(
            request.idempotency_key(),
            None,
            "{} must not also offer an idempotency key",
            request.name()
        );
        let json = serde_json::to_value(&request).expect("json");
        assert_eq!(
            json["clientOperationId"],
            serde_json::Value::String(format!("op-{}", json["id"].as_u64().expect("id"))),
            "{} must carry its own clientOperationId",
            request.name()
        );
    }
}

#[test]
fn a_queue_reply_says_whether_it_was_the_first_effect() {
    let json = serde_json::to_value(DaemonMessage::QueueAccepted {
        id: 7,
        replayed: false,
    })
    .expect("json");
    assert_eq!(json["type"], "queue_accepted");
    assert_eq!(json["id"], 7);
    assert_eq!(json["replayed"], false);
    assert_eq!(
        serde_json::from_value::<DaemonMessage>(json.clone()).expect("read back"),
        DaemonMessage::QueueAccepted {
            id: 7,
            replayed: false
        }
    );
    // Both answers are the same shape: a client reads `replayed`, never the
    // absence of a field.
    let replayed = serde_json::from_value::<DaemonMessage>(
        serde_json::to_value(DaemonMessage::QueueAccepted {
            id: 7,
            replayed: true,
        })
        .expect("json"),
    )
    .expect("read back");
    assert_eq!(
        replayed,
        DaemonMessage::QueueAccepted {
            id: 7,
            replayed: true
        }
    );
}

#[test]
fn the_snapshot_event_names_its_epoch_revision_and_rows() {
    let event = SessionEvent::QueueSnapshot {
        epoch: "0123456789abcdef0123456789abcdef".to_string(),
        revision: 7,
        items: vec![QueuedMessage {
            item_id: "queue-3".to_string(),
            text: "later".to_string(),
            attachment_references: vec![AttachmentReference {
                session_id: "s.a.1".to_string(),
                digest: "a".repeat(64),
                stored_bytes: 12,
            }],
            error: Some("refused".to_string()),
        }],
        dropped: Vec::new(),
    };
    let json = serde_json::to_value(&event).expect("json");
    assert_eq!(json["type"], "queue_snapshot");
    assert_eq!(json["epoch"], "0123456789abcdef0123456789abcdef");
    assert_eq!(json["revision"], 7);
    assert_eq!(json["items"][0]["itemId"], "queue-3");
    assert_eq!(
        json["items"][0]["attachmentReferences"][0]["digest"],
        "a".repeat(64)
    );
    assert_eq!(json["items"][0]["error"], "refused");
    // The envelope carries the session, so the payload does not repeat it.
    assert!(
        json.get("sessionId").is_none(),
        "the envelope's sessionId is the only session authority on this event"
    );
    assert!(
        json.get("dropped").is_none(),
        "an ordinary snapshot omits the dropped list entirely"
    );
    assert_eq!(
        serde_json::from_value::<SessionEvent>(json).expect("read back"),
        event
    );
    assert_eq!(event.kind(), "queue_snapshot");
}

/// An empty queue is the common case at attach, so the row list and the error
/// both have to be omissible without the event reading as broken.
#[test]
fn an_empty_snapshot_round_trips_without_optional_row_fields() {
    let event = SessionEvent::QueueSnapshot {
        epoch: "0123456789abcdef0123456789abcdef".to_string(),
        revision: 1,
        items: Vec::new(),
        dropped: Vec::new(),
    };
    let json = serde_json::to_value(&event).expect("json");
    assert_eq!(json["items"], serde_json::json!([]));
    assert_eq!(
        serde_json::from_value::<SessionEvent>(json).expect("read back"),
        event
    );
}

#[test]
fn a_dropped_row_names_the_item_and_the_reason_and_nothing_else() {
    let json = serde_json::to_value(&SessionEvent::QueueSnapshot {
        epoch: "0123456789abcdef0123456789abcdef".to_string(),
        revision: 3,
        items: Vec::new(),
        dropped: vec![DroppedQueuedMessage {
            item_id: "queue-1".to_string(),
            reason: DroppedReason::DeliveryUnknown,
        }],
    })
    .expect("json");
    let dropped = json["dropped"].as_array().expect("dropped list");
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0]["itemId"], "queue-1");
    assert_eq!(dropped[0]["reason"], "delivery_unknown");
    assert_eq!(
        dropped[0].as_object().expect("object").len(),
        2,
        "a dropped entry carries no text and no attachments: the client has them"
    );
}

#[test]
fn the_daemon_and_the_client_both_offer_the_queue_capability() {
    assert!(m3a_daemon_capabilities()
        .iter()
        .any(|cap| cap.as_str() == caps::SESSION_QUEUE));
    assert!(m3a_client_capabilities()
        .iter()
        .any(|cap| cap.as_str() == caps::SESSION_QUEUE));
    assert!(
        intersect_capabilities(&m3a_client_capabilities(), &m3a_daemon_capabilities())
            .iter()
            .any(|cap| cap.as_str() == caps::SESSION_QUEUE)
    );
    // The two lists are equal apart from the names the daemon alone serves;
    // `tests::daemon_and_client_advertise_sessions` is where that is pinned,
    // as the client list plus a named set.
}

#[test]
fn the_queue_frames_are_gated_by_a_capability_the_floor_does_not_move_for() {
    // The queue frames are new variants, so a v21 daemon's reader cannot
    // deserialize them; `session.queue` is what stops a client sending one.
    const {
        assert!(
            PROTOCOL_VERSION >= 22,
            "protocol 22 is the dialect that added the queue frames"
        );
    }
    assert_eq!(
        PROTOCOL_MIN_VERSION, 16,
        "the floor moves only for a required field"
    );
}

#[test]
fn the_interrupt_behaviour_is_a_spelling_a_peer_can_still_read() {
    // No client sends it, but the daemon's own send-now does, so the spelling
    // has to be the one the wire's own rename rule produces.
    let json = serde_json::to_string(&ActiveTurnBehavior::Interrupt).expect("serialize");
    assert_eq!(json, "\"interrupt\"");
    assert_eq!(
        serde_json::from_str::<ActiveTurnBehavior>(&json).expect("deserialize"),
        ActiveTurnBehavior::Interrupt
    );
}
