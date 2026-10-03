#![cfg(windows)]

//! What the attach reply's `resume` does on the reader thread: a reset reaches
//! the subscriber before any frame that follows the reply, and a plain resume
//! reaches it as nothing at all.
//!
//! The order is the whole contract. The reader thread is the only thing that
//! dispatches a subscription's envelopes, so the reply is matched there and the
//! hook runs on that thread before the reply is handed to the waiting call —
//! strictly before the next frame off the socket, which no other thread can
//! overtake.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_protocol::{
    ClientMessage, Cursor, DaemonMessage, SessionEvent, SessionEventEnvelope, SessionResumeInfo,
    SessionResumeOutcome, SessionResumeReason, SessionResumeTail,
};

use super::tests::with_a_fake_daemon;
use crate::client::DaemonClient;
use crate::error::DaemonError;

/// One recorded delivery, in the order this connection delivered them.
type Seen = Arc<Mutex<Vec<String>>>;

fn reset(reason: SessionResumeReason) -> SessionResumeInfo {
    SessionResumeInfo {
        resume: SessionResumeOutcome::Reset {
            reason,
            tail: SessionResumeTail {
                cursor: Cursor {
                    generation: 2,
                    seq: 7,
                },
                events: Vec::new(),
                tail_complete: false,
            },
        },
        oldest_seq: 1,
        head: 7,
    }
}

fn resumed() -> SessionResumeInfo {
    SessionResumeInfo {
        resume: SessionResumeOutcome::Resumed,
        oldest_seq: 1,
        head: 7,
    }
}

fn agent_frame(subscription_id: u64, text: &str) -> DaemonMessage {
    DaemonMessage::SubscriptionEvent {
        subscription_id,
        envelope: SessionEventEnvelope {
            session_id: "s.resume".to_string(),
            generation: 2,
            transcript_seq: None,
            event: SessionEvent::AgentMessage {
                message_id: None,
                text: text.to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
        },
    }
}

fn reason_name(reason: SessionResumeReason) -> &'static str {
    match reason {
        SessionResumeReason::EpochChanged => "epoch_changed",
        SessionResumeReason::CursorAhead => "cursor_ahead",
        SessionResumeReason::CursorCompacted => "cursor_compacted",
        SessionResumeReason::JournalGap => "journal_gap",
    }
}

/// Attach with a cursor the daemon has an answer about, recording both lanes
/// into one list so their order is the list's order.
fn attach(client: &DaemonClient, seen: Seen) -> Result<u64, DaemonError> {
    let events = Arc::clone(&seen);
    let resets = Arc::clone(&seen);
    client.session_attach_with_subscription(
        7,
        "s.resume",
        Some(Cursor {
            generation: 1,
            seq: 3,
        }),
        Arc::new(move |envelope| {
            let SessionEvent::AgentMessage { text, .. } = &envelope.event else {
                return;
            };
            events
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(format!("row:{text}"));
        }),
        Some(Arc::new(move |info| {
            let SessionResumeOutcome::Reset { reason, .. } = &info.resume else {
                return;
            };
            resets
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(format!("reset:{}", reason_name(*reason)));
        })),
    )
}

/// The body must not return while replay frames are still in flight: dropping
/// the client then breaks the server's remaining sends with a broken pipe.
fn wait_for_entries(seen: &Seen, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let entries = seen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if entries.len() >= expected {
            return;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for {expected} deliveries, saw {entries:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Reads one attach, answers it, and pushes the frames that follow — in that
/// order, which is what makes the assertion about delivery order meaningful.
fn serve_attach(
    resume: SessionResumeInfo,
    replayed: &[&str],
) -> impl FnOnce(crate::framing::Framed) + Send + 'static {
    let replayed = replayed
        .iter()
        .map(|text| (*text).to_string())
        .collect::<Vec<_>>();
    move |framed| {
        let ClientMessage::SessionAttach {
            id,
            subscription_id,
            ..
        } = framed.recv::<ClientMessage>().expect("attach request")
        else {
            panic!("expected an attach request");
        };
        framed
            .send(&DaemonMessage::SessionAttached {
                id,
                subscription_id,
                resume: Some(resume),
            })
            .expect("attach reply");
        for text in &replayed {
            framed
                .send(&agent_frame(subscription_id, text))
                .expect("replayed frame");
        }
        let _ = framed.recv_timeout::<ClientMessage>(Duration::from_secs(10));
    }
}

#[cfg(windows)]
#[test]
fn a_reset_reply_reaches_the_subscriber_before_the_first_replayed_envelope() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let seen_by_attach = Arc::clone(&seen);
    let seen_by_wait = Arc::clone(&seen);
    with_a_fake_daemon(
        "client-resume-order",
        serve_attach(
            reset(SessionResumeReason::EpochChanged),
            &["replayed one", "replayed two"],
        ),
        move |client| {
            attach(client, seen_by_attach).expect("attach with a stale cursor");
            wait_for_entries(&seen_by_wait, 3);
        },
    );
    assert_eq!(
        *seen.lock().unwrap_or_else(|error| error.into_inner()),
        [
            "reset:epoch_changed",
            "row:replayed one",
            "row:replayed two"
        ],
        "the reset must reach the channel before any frame of that attach"
    );
}

/// A `resumed` outcome names no tail, so the hook is not called at all and the
/// replay that follows is the only thing the subscriber sees.
#[cfg(windows)]
#[test]
fn a_resumed_reply_calls_no_hook() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let seen_by_attach = Arc::clone(&seen);
    let seen_by_wait = Arc::clone(&seen);
    with_a_fake_daemon(
        "client-resume-ok",
        serve_attach(resumed(), &["replayed one"]),
        move |client| {
            attach(client, seen_by_attach).expect("attach with a live cursor");
            wait_for_entries(&seen_by_wait, 1);
        },
    );
    assert_eq!(
        *seen.lock().unwrap_or_else(|error| error.into_inner()),
        ["row:replayed one"],
        "nothing is reset, so nothing but the replay is delivered"
    );
}
