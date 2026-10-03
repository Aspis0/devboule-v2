//! What a reset does to the attachment registry: it reaches the sink ahead of
//! the frames that follow it, the entry continues from the tail's cursor, and
//! the reset itself attaches nothing a second time.
//!
//! Whether the sink's reader then replaces its rows is that reader's own
//! contract, not this one's — the registry's half is the order of the message,
//! the cursor, and the absence of a second attach.

use super::*;
use devboule_protocol::{
    Cursor, SessionResumeInfo, SessionResumeOutcome, SessionResumeReason, SessionResumeTail,
};

/// A sink that records both lanes into one list, so the reset reads in the
/// position the view receives it in.
fn recording_sink(seen: Arc<Mutex<Vec<String>>>) -> AttachmentSink {
    let events = Arc::clone(&seen);
    let resets = Arc::clone(&seen);
    AttachmentSink {
        events: Arc::new(move |event| {
            let SessionEvent::AgentMessage { text, .. } = event else {
                return;
            };
            events
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(format!("row:{text}"));
        }),
        reset: Arc::new(move |reset| {
            let SessionResumeOutcome::Reset { reason, tail } = &reset.resume else {
                return;
            };
            resets
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(format!("reset:{reason:?}/{} rows", tail.events.len()));
        }),
    }
}

/// The answer a daemon gives one attach: a reset carrying this tail and naming
/// the cursor the client continues from.
fn reset_reply(tail_rows: &[&str], cursor: Cursor) -> SessionResumeInfo {
    SessionResumeInfo {
        resume: SessionResumeOutcome::Reset {
            reason: SessionResumeReason::EpochChanged,
            tail: SessionResumeTail {
                cursor,
                events: tail_rows
                    .iter()
                    .map(|text| SessionEvent::AgentMessage {
                        message_id: None,
                        text: (*text).to_string(),
                        parent_tool_use_id: None,
                        spawn_depth: None,
                    })
                    .collect(),
                tail_complete: false,
            },
        },
        oldest_seq: 1,
        head: cursor.seq,
    }
}

fn recorded(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    seen.lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

fn cursor_of(registry: &AttachmentRegistry, subscription_id: SubscriptionId) -> Option<Cursor> {
    registry
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .entries
        .get(&subscription_id)
        .and_then(|entry| entry.cursor)
}

fn asked_cursor(client: &FakeAttachmentClient, at: usize) -> Option<Cursor> {
    client
        .calls
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(at)
        .and_then(|(_, cursor)| *cursor)
}

#[test]
fn a_reset_reaches_the_sink_before_the_first_frame_of_that_attach() {
    let registry = Arc::new(AttachmentRegistry::default());
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let subscription_id = registry.insert("s.reset", None, recording_sink(Arc::clone(&seen)));
    let old_client = FakeAttachmentClient::default();
    let new_client = FakeAttachmentClient::default();
    registry
        .bind(&old_client, subscription_id)
        .expect("initial attach");
    for text in ["before one", "before two"] {
        old_client.emit("s.reset", agent_envelope("s.reset", 1, Some(17), text));
    }

    new_client.answer_attach_with(
        "s.reset",
        reset_reply(
            &["tail row"],
            Cursor {
                generation: 2,
                seq: 7,
            },
        ),
    );
    registry.begin_replacement();
    registry.reattach_all(&new_client);
    new_client.emit("s.reset", agent_envelope("s.reset", 2, Some(8), "live row"));

    assert_eq!(
        recorded(&seen),
        [
            "row:before one",
            "row:before two",
            "reset:EpochChanged/1 rows",
            "row:live row",
        ],
        "the reset carries the tail and lands ahead of the replay of that attach"
    );
    assert_eq!(
        new_client
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len(),
        1,
        "the tail is in the answer, so a reset must never attach again"
    );
}

#[test]
fn the_next_reattach_after_a_reset_asks_from_the_tails_cursor() {
    let registry = Arc::new(AttachmentRegistry::default());
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let subscription_id = registry.insert("s.reset", None, recording_sink(seen));
    let old_client = FakeAttachmentClient::default();
    let new_client = FakeAttachmentClient::default();
    registry
        .bind(&old_client, subscription_id)
        .expect("initial attach");
    old_client.emit("s.reset", agent_envelope("s.reset", 1, Some(17), "before"));

    let tail_cursor = Cursor {
        generation: 2,
        seq: 7,
    };
    new_client.answer_attach_with("s.reset", reset_reply(&["tail row"], tail_cursor));
    registry.begin_replacement();
    registry.reattach_all(&new_client);
    assert_eq!(
        cursor_of(&registry, subscription_id),
        Some(tail_cursor),
        "the entry continues from the tail, not from where the reset was taken"
    );

    let later_client = FakeAttachmentClient::default();
    registry.begin_replacement();
    registry.reattach_all(&later_client);
    assert_eq!(
        asked_cursor(&later_client, 0),
        Some(Cursor {
            generation: 2,
            seq: 6,
        }),
        "the reattach backs one envelope off the tail cursor, as it does off \
         every other cursor"
    );
}
