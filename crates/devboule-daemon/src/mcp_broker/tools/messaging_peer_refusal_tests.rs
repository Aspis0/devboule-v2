//! Tests for who a paired-device send may reach and what the caller hears
//! when the far side will not take it (`messaging_peer.rs`). The fixtures
//! they dial with live in the sibling `tests` module.

use std::time::Duration;

use devboule_protocol::{AgentMessageState, ClientMessage, DaemonMessage, ErrorCode, WireError};
use serde_json::json;

use super::tests::{
    call, pinned_responder, refusal, registration, remote_send_capability, row, USER,
};
use crate::server::ServerState;

/// A device paired by another user is not this session's to call, and the
/// refusal matches the roster tool's word for word. Nothing is dialled: the
/// stored address is a closed port, so a dial that happened would answer
/// with the connect failure instead of the pairing sentence.
#[test]
fn a_device_paired_by_another_user_refuses_by_name() {
    let state = ServerState::new("peer-send-other-user".to_string());
    state
        .peer_upsert(row("dev-other", "127.0.0.1:1", Some("S-someone-else")))
        .expect("the row");
    let answer = call(
        &state,
        &registration("s.local.sender"),
        json!(1),
        "dev-other",
        "s.far.1",
        "hi",
    );
    let sentence = refusal(&answer);
    assert!(
        sentence.contains("No paired device named 'dev-other'"),
        "{answer}"
    );
    assert!(
        !sentence.contains("did not complete the call"),
        "another user's device is refused before any dial: {answer}"
    );
}

/// A pairing that recorded no user cannot be attributed to this session, and
/// its own refusal says that — not the absent-device sentence.
#[test]
fn a_pairing_without_a_recorded_user_is_its_own_refusal() {
    let state = ServerState::new("peer-send-anon".to_string());
    state
        .peer_upsert(row("dev-anon", "127.0.0.1:1", None))
        .expect("the row");
    let answer = call(
        &state,
        &registration("s.local.sender"),
        json!(1),
        "dev-anon",
        "s.far.1",
        "hi",
    );
    let sentence = refusal(&answer);
    assert!(sentence.contains("no user recorded"), "{answer}");
    assert!(!sentence.contains("No paired device named"), "{answer}");
}

/// A revoked pairing is a gone pairing, a different fact from absence, and
/// only re-pairing brings it back.
#[test]
fn a_revoked_pairing_refuses_as_gone() {
    let state = ServerState::new("peer-send-revoked".to_string());
    let mut record = row("dev-gone", "127.0.0.1:1", Some(USER));
    record.revoked_at = Some(99);
    state.peer_upsert(record).expect("the row");
    let answer = call(
        &state,
        &registration("s.local.sender"),
        json!(1),
        "dev-gone",
        "s.far.1",
        "hi",
    );
    let sentence = refusal(&answer);
    assert!(sentence.contains("pairing"), "{answer}");
    assert!(sentence.contains("gone"), "{answer}");
}

/// A far daemon that does not advertise the remote-send frame cannot read
/// the request, so the dial refuses before it leaves: nothing reaches the far
/// side at all.
#[test]
fn a_far_end_that_cannot_speak_remote_send_sees_no_request() {
    let state = ServerState::new("peer-send-unsupported".to_string());
    let requests = pinned_responder(
        &state,
        "dev-old",
        Some(USER),
        Vec::new(),
        DaemonMessage::AgentMessageReceipt {
            id: 0,
            state: AgentMessageState::Accepted,
        },
        1,
    );
    let answer = call(
        &state,
        &registration("s.local.sender"),
        json!(1),
        "dev-old",
        "s.far.1",
        "hi",
    );
    let sentence = refusal(&answer);
    assert!(
        sentence.contains("predates a required peer capability"),
        "{answer}"
    );
    assert!(
        sentence.contains("not sent"),
        "a failure before the request leaves must say nothing was sent: {answer}"
    );
    let report = requests
        .recv_timeout(Duration::from_secs(10))
        .expect("the responder reports the dial");
    assert!(
        report.is_none(),
        "a frame the far end cannot speak must not leave this machine: {report:?}"
    );
}

/// The far gate's own refusal — a device this daemon pinned whose grant lacks
/// `send` — reaches the caller with the far daemon's sentence, not as an
/// unexpected message.
#[test]
fn a_far_gate_refusal_reaches_the_caller_as_its_own_sentence() {
    let state = ServerState::new("peer-send-far-gate".to_string());
    let requests = pinned_responder(
        &state,
        "dev-narrow",
        Some(USER),
        remote_send_capability(),
        DaemonMessage::Error(
            devboule_protocol::WireError::new(
                ErrorCode::CapabilityNotSupported,
                "capability 'send' was not negotiated",
            )
            .with_id(0),
        ),
        1,
    );
    let answer = call(
        &state,
        &registration("s.local.sender"),
        json!(1),
        "dev-narrow",
        "s.far.1",
        "hi",
    );
    let request = requests
        .recv_timeout(Duration::from_secs(10))
        .expect("the responder reports the dial")
        .expect("the dialer sent its request");
    assert!(
        matches!(request, ClientMessage::AgentMessageSend { .. }),
        "the refusal is the far gate's answer to the frame: {request:?}"
    );
    let sentence = refusal(&answer);
    assert!(
        sentence.contains("capability 'send' was not negotiated"),
        "the far daemon's sentence is the answer: {answer}"
    );
    assert!(sentence.contains("dev-narrow"), "{answer}");
    assert!(
        sentence.contains("refused"),
        "a gate denial is a refusal: {answer}"
    );
}

/// A far idempotency conflict names the conflict and the key it collided
/// on — never "refused", which would invite a resend of the same key.
#[test]
fn a_far_idempotency_conflict_names_the_conflict_and_the_key() {
    let state = ServerState::new("peer-send-far-conflict".to_string());
    let requests = pinned_responder(
        &state,
        "dev-twice",
        Some(USER),
        remote_send_capability(),
        DaemonMessage::Error(
            WireError::new(
                ErrorCode::IdempotencyConflict,
                "idempotency key reused with a different payload",
            )
            .with_id(0),
        ),
        1,
    );
    let answer = call(
        &state,
        &registration("s.local.sender"),
        json!(1),
        "dev-twice",
        "s.far.1",
        "hi",
    );
    let request = requests
        .recv_timeout(Duration::from_secs(10))
        .expect("the responder reports the dial")
        .expect("the dialer sent its request");
    let key = match request {
        ClientMessage::AgentMessageSend {
            idempotency_key, ..
        } => idempotency_key.expect("every send carries a key"),
        other => panic!("expected AgentMessageSend, got {other:?}"),
    };
    let sentence = refusal(&answer);
    assert!(sentence.contains("idempotency conflict"), "{answer}");
    assert!(
        sentence.contains(&key),
        "the conflict names the key it collided on: {answer}"
    );
    assert!(
        !sentence.contains("refused"),
        "a conflict is not a refusal: {answer}"
    );
}

/// A far error that is neither a refusal nor a conflict names its code, so
/// the caller hears what kind of failure this was.
#[test]
fn an_unexpected_far_error_names_its_code() {
    let state = ServerState::new("peer-send-far-error".to_string());
    let requests = pinned_responder(
        &state,
        "dev-broken",
        Some(USER),
        remote_send_capability(),
        DaemonMessage::Error(
            WireError::new(ErrorCode::Internal, "the far daemon failed").with_id(0),
        ),
        1,
    );
    let answer = call(
        &state,
        &registration("s.local.sender"),
        json!(1),
        "dev-broken",
        "s.far.1",
        "hi",
    );
    let request = requests
        .recv_timeout(Duration::from_secs(10))
        .expect("the responder reports the dial")
        .expect("the dialer sent its request");
    assert!(
        matches!(request, ClientMessage::AgentMessageSend { .. }),
        "the error is the far side's answer to the frame: {request:?}"
    );
    let sentence = refusal(&answer);
    assert!(sentence.contains("internal"), "{answer}");
    assert!(
        sentence.contains("the far daemon failed"),
        "the far daemon's sentence is the answer: {answer}"
    );
    assert!(
        !sentence.contains("refused"),
        "a non-refusal error must not borrow the word: {answer}"
    );
}
