//! Tests for what a paired-device send carries and answers
//! (`messaging_peer.rs`), kept out of the production file: the canned
//! responders and fixture rows are test-only weight. The refusal cases and
//! the fixtures they borrow live in the sibling `refusal_tests` module.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use devboule_protocol::{AgentMessageState, ClientMessage, DaemonMessage, OwnerId};
use serde_json::{json, Value};

use crate::journal::PeerRecord;
use crate::mcp_broker::caller::McpCaller;
use crate::mcp_broker::tools::messaging;
use crate::mcp_broker::RegisteredSession;
use crate::provider_catalog::ToolOverlay;
use crate::server::ServerState;

pub(in crate::mcp_broker) const USER: &str = "S-1-5-21-peer-send";

pub(in crate::mcp_broker) fn owner() -> OwnerId {
    OwnerId::new(USER, "claude").expect("owner")
}

pub(super) fn registration(session_id: &str) -> RegisteredSession {
    RegisteredSession {
        session_id: session_id.to_string(),
        owner: owner(),
        provider_id: Some("claude".to_string()),
        depth: 0,
        overlay: ToolOverlay::NONE,
        bearer: "the bearer".to_string(),
        claude_config_path: None,
        runtime: None,
        broker_ready: Arc::new(AtomicBool::new(false)),
    }
}

pub(super) fn row(device_id: &str, address: &str, paired_by: Option<&str>) -> PeerRecord {
    PeerRecord {
        device_id: device_id.to_string(),
        display_name: format!("Device {device_id}"),
        role: "daemon".to_string(),
        public_key: vec![7u8; 32],
        paired_by_user: paired_by.map(str::to_string),
        binding_kind: "tailnet".to_string(),
        binding_stable_id: None,
        binding_node_name: None,
        binding_login_name: None,
        address: address.to_string(),
        paired_at: 1,
        revoked_at: None,
        caps: vec!["view".to_string(), "send".to_string()],
    }
}

pub(in crate::mcp_broker) fn remote_send_capability() -> Vec<devboule_protocol::Capability> {
    vec![devboule_protocol::Capability::new(
        devboule_protocol::caps::AGENT_MESSAGES,
    )]
}

/// Pin one loopback dial target into `device_id`'s row: a Noise responder
/// advertising `capabilities`, answering `reply` to every request, and
/// reporting each dial's request — or `None` for a dial that sent none.
pub(in crate::mcp_broker) fn pinned_responder(
    state: &Arc<ServerState>,
    device_id: &str,
    paired_by: Option<&str>,
    capabilities: Vec<devboule_protocol::Capability>,
    reply: DaemonMessage,
    dials: usize,
) -> mpsc::Receiver<Option<ClientMessage>> {
    let keypair = snow::Builder::new(
        crate::peer_transport::PEER_NOISE_PATTERN
            .parse()
            .expect("pattern"),
    )
    .generate_keypair()
    .expect("keypair");
    let private: [u8; 32] = keypair.private.clone().try_into().expect("32 bytes");
    let (address, requests) =
        crate::test_support::spawn_capturing_noise_responder(private, capabilities, reply, dials);
    let mut record = row(device_id, &address.to_string(), paired_by);
    record.public_key = keypair.public.clone();
    state.peer_upsert(record).expect("the row");
    requests
}

/// Pin one loopback dial target into `device_id`'s row backed by a silent
/// responder: handshake, hello, the request reported, then no reply.
pub(super) fn pinned_silent_responder(
    state: &Arc<ServerState>,
    device_id: &str,
    paired_by: Option<&str>,
) -> mpsc::Receiver<Option<ClientMessage>> {
    let keypair = snow::Builder::new(
        crate::peer_transport::PEER_NOISE_PATTERN
            .parse()
            .expect("pattern"),
    )
    .generate_keypair()
    .expect("keypair");
    let private: [u8; 32] = keypair.private.clone().try_into().expect("32 bytes");
    let (address, requests) =
        crate::test_support::spawn_silent_noise_responder(private, remote_send_capability());
    let mut record = row(device_id, &address.to_string(), paired_by);
    record.public_key = keypair.public.clone();
    state.peer_upsert(record).expect("the row");
    requests
}

/// One `devboule_send_message` call with a device target, as the broker
/// hands it to the tool body.
pub(super) fn call(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    id: Value,
    device_id: &str,
    to_agent: &str,
    text: &str,
) -> Value {
    messaging::send(
        state,
        registration,
        McpCaller::Local,
        id,
        &json!({"params": {"arguments": {
            "deviceId": device_id,
            "to_agent": to_agent,
            "text": text,
        }}}),
    )
    .expect("the handler does not fail")
    .expect("the handler answers")
}

pub(super) fn refusal(answer: &Value) -> String {
    answer
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a refusal, got {answer}"))
        .to_string()
}

/// The paired-device road, end to end over a real dial: the request carries
/// the caller's own session id as its source and the roster's session id as
/// its target, and the far daemon's receipt is the tool's answer — a
/// `rejected_absent` is not rewritten into anything friendlier.
#[test]
fn a_send_to_a_paired_device_carries_the_local_source_and_answers_the_far_receipt() {
    let state = ServerState::new("peer-send-receipt".to_string());
    let requests = pinned_responder(
        &state,
        "dev-far",
        Some(USER),
        remote_send_capability(),
        DaemonMessage::AgentMessageReceipt {
            id: 0,
            state: AgentMessageState::RejectedAbsent,
        },
        1,
    );
    let answer = call(
        &state,
        &registration("s.local.sender"),
        json!(3),
        "dev-far",
        "s.far.7",
        "hello from here",
    );

    let request = requests
        .recv_timeout(Duration::from_secs(10))
        .expect("the responder reports the dial")
        .expect("the dialer sent its request");
    let key = match request {
        ClientMessage::AgentMessageSend {
            from_session,
            to_session,
            text,
            idempotency_key,
            ..
        } => {
            assert_eq!(
                from_session, "s.local.sender",
                "the source is the caller's own session"
            );
            assert_eq!(
                to_session, "s.far.7",
                "the target is the roster's session id for that device"
            );
            assert_eq!(text, "hello from here");
            let key = idempotency_key.expect("every send carries a fresh key");
            assert!(
                key.starts_with("mcp-peer-send-"),
                "the key is a fresh random identity: {key}"
            );
            key
        }
        other => panic!("expected AgentMessageSend, got {other:?}"),
    };
    assert_eq!(
        answer.pointer("/result/structuredContent/state"),
        Some(&json!("rejected_absent")),
        "the far receipt is the answer, not a local 'accepted': {answer}"
    );
    assert_eq!(
        answer.pointer("/result/structuredContent/idempotencyKey"),
        Some(&json!(key)),
        "the answer traces which send this outcome belongs to: {answer}"
    );
    let text = answer
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a result text, got {answer}"));
    assert!(text.contains("rejected_absent"), "{answer}");
    assert!(
        text.contains(&key),
        "the result text carries the key: {answer}"
    );
    assert_eq!(
        answer.pointer("/result/isError"),
        Some(&json!(false)),
        "{answer}"
    );
}

/// Two calls — even with the same `tools/call` frame id — are two messages
/// with different keys: frame ids restart with the bridge process, so they
/// cannot dedupe. Both deliver.
#[test]
fn two_calls_with_the_same_frame_id_send_two_messages_with_different_keys() {
    let state = ServerState::new("peer-send-fresh-key".to_string());
    let requests = pinned_responder(
        &state,
        "dev-fresh",
        Some(USER),
        remote_send_capability(),
        DaemonMessage::AgentMessageReceipt {
            id: 0,
            state: AgentMessageState::Accepted,
        },
        2,
    );
    let registration = registration("s.local.sender");
    let first = call(
        &state,
        &registration,
        json!(9),
        "dev-fresh",
        "s.far.9",
        "one delivery",
    );
    let second = call(
        &state,
        &registration,
        json!(9),
        "dev-fresh",
        "s.far.9",
        "one delivery",
    );
    for answer in [&first, &second] {
        assert_eq!(
            answer.pointer("/result/structuredContent/state"),
            Some(&json!("accepted")),
            "{answer}"
        );
    }

    let mut keys = Vec::new();
    for _ in 0..2 {
        let request = requests
            .recv_timeout(Duration::from_secs(10))
            .expect("the responder reports the dial")
            .expect("the dialer sent its request");
        match request {
            ClientMessage::AgentMessageSend {
                idempotency_key, ..
            } => keys.push(idempotency_key.expect("every send carries a key")),
            other => panic!("expected AgentMessageSend, got {other:?}"),
        }
    }
    assert_ne!(
        keys[0], keys[1],
        "a repeated call is a new message, not a replay"
    );
    for (answer, key) in [(&first, &keys[0]), (&second, &keys[1])] {
        assert_eq!(
            answer.pointer("/result/structuredContent/idempotencyKey"),
            Some(&json!(key)),
            "each answer traces its own send: {answer}"
        );
    }
}

/// An empty device id is a caller error, never a silent local send: a
/// message meant for another machine must not land on this one.
#[test]
fn an_empty_device_id_is_refused_not_read_as_a_local_send() {
    let state = ServerState::new("peer-send-empty-device".to_string());
    let answer = messaging::send(
        &state,
        &registration("s.local.sender"),
        McpCaller::Local,
        json!(1),
        &json!({"params": {"arguments": {
            "deviceId": "",
            "to_agent": "nobody",
            "text": "hi",
        }}}),
    )
    .expect("the handler does not fail")
    .expect("the handler answers");
    let sentence = refusal(&answer);
    assert!(sentence.contains("deviceId"), "{answer}");
    assert!(
        !sentence.contains("target agent not found"),
        "an empty device id must not fall back to the local lookup: {answer}"
    );
}

/// An explicit `deviceId: null` names no device and selects no road: only
/// an absent property keeps the local send. Null is the same caller error
/// as an empty id.
#[test]
fn an_explicit_null_device_id_is_refused_not_read_as_a_local_send() {
    let state = ServerState::new("peer-send-null-device".to_string());
    let answer = messaging::send(
        &state,
        &registration("s.local.sender"),
        McpCaller::Local,
        json!(1),
        &json!({"params": {"arguments": {
            "deviceId": null,
            "to_agent": "nobody",
            "text": "hi",
        }}}),
    )
    .expect("the handler does not fail")
    .expect("the handler answers");
    let sentence = refusal(&answer);
    assert!(sentence.contains("deviceId"), "{answer}");
    assert!(
        !sentence.contains("target agent not found"),
        "an explicit null must not fall back to the local lookup: {answer}"
    );
}

/// A wrong-typed `deviceId` (here a number) cannot name a device either:
/// caller error, never a quiet local send.
#[test]
fn a_non_string_device_id_is_refused_not_read_as_a_local_send() {
    let state = ServerState::new("peer-send-number-device".to_string());
    let answer = messaging::send(
        &state,
        &registration("s.local.sender"),
        McpCaller::Local,
        json!(1),
        &json!({"params": {"arguments": {
            "deviceId": 7,
            "to_agent": "nobody",
            "text": "hi",
        }}}),
    )
    .expect("the handler does not fail")
    .expect("the handler answers");
    let sentence = refusal(&answer);
    assert!(sentence.contains("deviceId"), "{answer}");
    assert!(
        !sentence.contains("target agent not found"),
        "a wrong-typed device id must not fall back to the local lookup: {answer}"
    );
}

/// A far end that takes the request and goes quiet: no reply comes back, so
/// the delivery fate is unknown. The answer says so with the wire key,
/// instead of inviting a resend that could deliver twice.
#[test]
fn a_far_end_that_goes_silent_answers_outcome_unknown_with_the_key() {
    let state = ServerState::new("peer-send-unknown".to_string());
    let requests = pinned_silent_responder(&state, "dev-quiet", Some(USER));
    let answer = call(
        &state,
        &registration("s.local.sender"),
        json!(4),
        "dev-quiet",
        "s.far.4",
        "hello?",
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
    assert!(
        sentence.contains("outcome is unknown"),
        "a reply that never comes is an unknown outcome: {answer}"
    );
    assert!(sentence.contains("may have been delivered"), "{answer}");
    assert!(sentence.contains("do not resend"), "{answer}");
    assert!(
        sentence.contains(&key),
        "the unknown outcome carries the wire key: {answer}"
    );
}
