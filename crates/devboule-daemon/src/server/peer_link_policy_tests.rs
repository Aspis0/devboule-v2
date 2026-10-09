//! What the remote machine decides about a held-link read: which capability
//! each list needs there, what a refusal looks like when it travels back, and
//! what happens when the far daemon is too old to speak the frame at all.
//!
//! The link never widens a grant: the local daemon asks, the remote's
//! `run_gate` answers, and the answer is carried through as the remote's own.

use std::time::Duration;

use devboule_protocol::{
    caps, Capability, ClientMessage, ErrorCode, PermissionOutcome, RemoteHostList, RemoteHostState,
    SessionKind, WireError,
};

use super::harness::Harness;
use super::peer_link_test_support::eventually;
use crate::peer_policy::{peer_allows, PeerDecision, CAP_ADMIN, CAP_VIEW};

use crate::server::peer_link_state::LinkAnswer;

/// The three reads, each under the capability that names it **on the remote**:
/// the session list is a view act, and the two inventories are administrative.
/// Pinned here because this is the only place the link's allowlist and the
/// remote's gate meet, so a read that changed side would silently change what
/// a pairing has to hold.
#[test]
fn each_list_carries_the_capability_the_remote_gate_names() {
    let view = vec![CAP_VIEW.to_string()];
    let admin = vec![CAP_VIEW.to_string(), CAP_ADMIN.to_string()];
    let expect = |list: RemoteHostList, caps: &[String], decision: PeerDecision| {
        let request = list.peer_request(1);
        assert_eq!(
            peer_allows(caps, &request),
            decision,
            "{} under {caps:?}",
            request.name()
        );
    };
    expect(RemoteHostList::Sessions, &view, PeerDecision::Allow);
    expect(RemoteHostList::Sessions, &[], PeerDecision::Deny(CAP_VIEW));
    expect(
        RemoteHostList::Projects,
        &view,
        PeerDecision::Deny(CAP_ADMIN),
    );
    expect(
        RemoteHostList::Workspaces {
            project_id: "p1".to_string(),
        },
        &view,
        PeerDecision::Deny(CAP_ADMIN),
    );
    expect(RemoteHostList::Projects, &admin, PeerDecision::Allow);
}

/// The three host frames are local-only: a peer cannot make this daemon dial a
/// third machine, and no capability opens that. Slice 4's nine operate
/// frames keep the same door: driving another machine's sessions through
/// this daemon would turn it into the same proxy.
#[test]
fn a_peer_may_not_reach_a_link() {
    for request in [
        ClientMessage::RemoteHostWatch {
            id: 1,
            device_id: "b".to_string(),
        },
        ClientMessage::RemoteHostUnwatch {
            id: 2,
            device_id: "b".to_string(),
        },
        ClientMessage::RemoteHostList {
            id: 3,
            device_id: "b".to_string(),
            list: RemoteHostList::Sessions,
        },
        ClientMessage::RemoteHostCreate {
            id: 4,
            device_id: "b".to_string(),
            workspace_id: Some("w.1".to_string()),
            kind: SessionKind::Terminal,
            provider: None,
            mode: None,
            display_name: None,
            idempotency_key: None,
            cols: None,
            rows: None,
        },
        ClientMessage::RemoteHostSend {
            id: 5,
            device_id: "b".to_string(),
            session_id: "s.a.1".to_string(),
            subscription_id: 1,
            text: "hi".to_string(),
            attachments: Vec::new(),
            active_turn_behavior: None,
            attachment_references: Vec::new(),
            idempotency_key: None,
        },
        ClientMessage::RemoteHostResize {
            id: 6,
            device_id: "b".to_string(),
            session_id: "s.a.1".to_string(),
            subscription_id: 1,
            cols: 80,
            rows: 24,
        },
        ClientMessage::RemoteHostClaim {
            id: 7,
            device_id: "b".to_string(),
            session_id: "s.a.1".to_string(),
            subscription_id: 1,
        },
        ClientMessage::RemoteHostInterrupt {
            id: 8,
            device_id: "b".to_string(),
            session_id: "s.a.1".to_string(),
            subscription_id: 1,
        },
        ClientMessage::RemoteHostPermissionRespond {
            id: 9,
            device_id: "b".to_string(),
            session_id: "s.a.1".to_string(),
            subscription_id: 1,
            request_id: "card-1".to_string(),
            outcome: PermissionOutcome::AllowOnce,
            option_id: None,
            answer: None,
            idempotency_key: None,
        },
        ClientMessage::RemoteHostClose {
            id: 10,
            device_id: "b".to_string(),
            session_id: "s.a.1".to_string(),
            idempotency_key: None,
        },
        ClientMessage::RemoteHostStop {
            id: 11,
            device_id: "b".to_string(),
            session_id: "s.a.1".to_string(),
            subscription_id: 1,
        },
        ClientMessage::RemoteHostProviders {
            id: 12,
            device_id: "b".to_string(),
        },
    ] {
        assert_eq!(
            peer_allows(&["view".to_string(), CAP_ADMIN.to_string()], &request),
            PeerDecision::Deny(caps::REMOTE_HOSTS),
            "{} must be refused by name",
            request.name()
        );
    }
}

/// A refusal is the far side's own answer: its code and its sentence travel
/// back untouched, so the sidebar's empty state can say what that machine said.
#[test]
fn a_refusal_surfaces_the_remotes_own_reason() {
    let harness = Harness::start("peer-link-refusal");
    harness.responder.set_refusal(WireError::new(
        ErrorCode::Unauthorized,
        "this pairing may not read the project inventory",
    ));
    harness.watch();
    eventually("the link comes up", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Online)
    });
    match harness.links.read("b", RemoteHostList::Projects) {
        LinkAnswer::Refused(error) => {
            assert_eq!(error.code, ErrorCode::Unauthorized);
            assert_eq!(
                error.message, "this pairing may not read the project inventory",
                "the remote's own sentence, not this daemon's paraphrase"
            );
        }
        other => panic!("expected the remote's refusal, got {other:?}"),
    }
}

/// A far daemon that predates the frame is refused before the request leaves:
/// its reader could not deserialize the request and the connection would die
/// on it. The host row says `unsupported`, the one state that names "update
/// this machine".
#[test]
fn a_far_daemon_without_the_frames_is_unsupported() {
    let harness =
        Harness::with_capabilities("peer-link-unsupported", vec![Capability::new(caps::PING)]);
    harness.watch();
    eventually("the link comes up", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Online)
    });
    for list in [
        RemoteHostList::Projects,
        RemoteHostList::Sessions,
        RemoteHostList::Workspaces {
            project_id: "p1".to_string(),
        },
    ] {
        match harness.links.read("b", list.clone()) {
            LinkAnswer::Failed(state, sentence) => {
                assert_eq!(
                    state,
                    RemoteHostState::Unsupported,
                    "{list:?} must name the version skew, not the network: {sentence}"
                );
            }
            other => panic!("expected {list:?} to be unsupported, got {other:?}"),
        }
    }
    assert!(
        harness.requests.try_recv().is_err(),
        "no request may reach a far end that cannot parse it"
    );
}

/// The vocabulary is three reads, closed: this match is exhaustive, so a fourth
/// variant has to be added here, which is what makes adding one a reviewable
/// act rather than a silent widening of what can reach a peer.
#[test]
fn the_link_reads_exactly_three_lists() {
    fn count(list: &RemoteHostList) -> usize {
        match list {
            RemoteHostList::Projects => 1,
            RemoteHostList::Workspaces { .. } => 1,
            RemoteHostList::Sessions => 1,
        }
    }
    let total = count(&RemoteHostList::Projects)
        + count(&RemoteHostList::Workspaces {
            project_id: "p1".to_string(),
        })
        + count(&RemoteHostList::Sessions);
    assert_eq!(total, 3, "the allowlist is three reads and no more");
}

/// Every published failure sentence is one plain English sentence, and none of
/// them carries an address, a key or a tailnet name: they are what the app
/// renders, so what they contain is a privacy property, not a style one.
#[test]
fn the_failure_sentences_name_no_address_and_no_key() {
    use super::{offline_sentence, sentence_for, unsupported_sentence};
    use crate::server::peer_dial::DialStep;
    let sentences = [
        offline_sentence(),
        unsupported_sentence(),
        &sentence_for(DialStep::RowMissing),
        &sentence_for(DialStep::Revoked),
        &sentence_for(DialStep::Identity),
        &sentence_for(DialStep::Connect),
        &sentence_for(DialStep::Handshake),
        &sentence_for(DialStep::Hello),
        &sentence_for(DialStep::Busy),
        &sentence_for(DialStep::Address),
        &sentence_for(DialStep::NoListenPort),
    ];
    for sentence in sentences {
        let lower = sentence.to_lowercase();
        for forbidden in [
            "100.", "fd7a", ":47831", "://", "key", "tailnet", "\n", "\r",
        ] {
            assert!(
                !lower.contains(forbidden),
                "{sentence:?} leaks {forbidden:?}"
            );
        }
        assert!(
            sentence.ends_with('.'),
            "{sentence:?} is one sentence, so it ends with a stop"
        );
    }
}

/// Slice 4: an operate create reaches the peer as its own `SessionCreate`,
/// with the workspace, kind, display name and idempotency key carried
/// through unchanged. The answer is the host's own session row.
#[test]
fn a_remote_create_is_the_peers_own_session_create() {
    let harness = Harness::start("peer-link-operate-create");
    harness.watch();
    eventually("the link comes up", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Online)
    });
    match harness.links.operate_create(
        "b",
        Some("far-workspace".to_string()),
        SessionKind::Terminal,
        None,
        None,
        Some("B shell".to_string()),
        Some("key-1".to_string()),
        Some(80),
        Some(24),
    ) {
        LinkAnswer::Created(session) => {
            assert_eq!(session.id, "far-session");
            assert_eq!(session.workspace_id.as_deref(), Some("far-workspace"));
            assert_eq!(session.kind, SessionKind::Terminal);
            assert_eq!(session.display_name.as_deref(), Some("B shell"));
        }
        other => panic!("expected the host's session, got {other:?}"),
    }
    match harness.requests.recv_timeout(Duration::from_secs(5)) {
        Ok(ClientMessage::SessionCreate {
            workspace_id,
            kind,
            display_name,
            idempotency_key,
            cols,
            rows,
            ..
        }) => {
            assert_eq!(workspace_id.as_deref(), Some("far-workspace"));
            assert_eq!(kind, SessionKind::Terminal);
            assert_eq!(display_name.as_deref(), Some("B shell"));
            assert_eq!(idempotency_key.as_deref(), Some("key-1"));
            assert_eq!((cols, rows), (Some(80), Some(24)));
        }
        Ok(other) => panic!("expected the peer's SessionCreate, got {other:?}"),
        Err(error) => panic!("the create never reached the peer: {error:?}"),
    }
}

/// Slice 4: every other operate call reaches the peer as its own session
/// frame, and the answers come back in their own shapes.
#[test]
fn remote_operate_calls_are_the_peers_own_session_frames() {
    let harness = Harness::start("peer-link-operate-frames");
    harness.watch();
    eventually("the link comes up", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Online)
    });
    match harness.links.operate_send(
        "b",
        "far-session".to_string(),
        1,
        "echo hi".to_string(),
        Vec::new(),
        None,
        Some("key-2".to_string()),
        Vec::new(),
    ) {
        LinkAnswer::Sent(turn_active) => assert!(turn_active),
        other => panic!("expected the send state, got {other:?}"),
    }
    for answer in [
        harness
            .links
            .operate_resize("b", "far-session".to_string(), 1, 80, 24),
        harness
            .links
            .operate_claim("b", "far-session".to_string(), 1),
        harness
            .links
            .operate_interrupt("b", "far-session".to_string(), 1),
        harness.links.operate_permission_respond(
            "b",
            "far-session".to_string(),
            1,
            "card-1".to_string(),
            PermissionOutcome::AllowOnce,
            None,
            None,
            Some("key-3".to_string()),
        ),
        harness
            .links
            .operate_close("b", "far-session".to_string(), Some("key-4".to_string())),
        harness
            .links
            .operate_stop("b", "far-session".to_string(), 1),
    ] {
        assert!(
            matches!(answer, LinkAnswer::Accepted),
            "an Ok-shaped operate call is Accepted, got {answer:?}"
        );
    }
    match harness.links.operate_providers("b") {
        LinkAnswer::Providers {
            providers,
            unreadable_dirs,
        } => {
            assert!(providers.is_empty());
            assert_eq!(unreadable_dirs, 0);
        }
        other => panic!("expected the host's catalog, got {other:?}"),
    }
}

/// Slice 4: a refusal of an operate call is the far side's own answer — its
/// code and its sentence travel back untouched. This is the shape a missing
/// `create_sessions` grant takes on the host: the host's gate refuses the
/// peer's own `SessionCreate`, and the link carries that refusal through.
#[test]
fn a_remote_refusal_on_an_operate_call_travels_back_intact() {
    let harness = Harness::start("peer-link-operate-refusal");
    harness.responder.set_refusal(WireError::new(
        ErrorCode::CapabilityNotSupported,
        "capability 'create_sessions' was not negotiated",
    ));
    harness.watch();
    eventually("the link comes up", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Online)
    });
    match harness.links.operate_create(
        "b",
        Some("far-workspace".to_string()),
        SessionKind::Terminal,
        None,
        None,
        None,
        None,
        None,
        None,
    ) {
        LinkAnswer::Refused(error) => {
            assert_eq!(error.code, ErrorCode::CapabilityNotSupported);
            assert_eq!(
                error.message, "capability 'create_sessions' was not negotiated",
                "the remote's own sentence, not this daemon's paraphrase"
            );
        }
        other => panic!("expected the remote's refusal, got {other:?}"),
    }
}

/// Slice 4: a revoked host refuses operate calls before dialing. The pairing
/// row is re-read on the way to every call, so a revoke that lands while the
/// link is up stops the next operate, not only the next reconnect.
#[test]
fn a_revoked_host_refuses_operate_calls_before_dialing() {
    let harness = Harness::start("peer-link-operate-revoked");
    harness.watch();
    eventually("the link comes up", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Online)
    });
    harness.links.revoke("b");
    match harness.links.operate_send(
        "b",
        "far-session".to_string(),
        1,
        "echo hi".to_string(),
        Vec::new(),
        None,
        None,
        Vec::new(),
    ) {
        LinkAnswer::Failed(state, _) => assert_eq!(
            state,
            RemoteHostState::NeedsPairing,
            "a revoked host is not offline: re-pairing is the repair"
        ),
        other => panic!("expected the pairing failure, got {other:?}"),
    }
}

/// Slice 4: the create mode travels with the peer's own `SessionCreate`, so
/// the host's mode gate can refuse it. A prompt-skipping mode is refused on
/// the host even though the link carried it faithfully.
#[test]
fn a_mode_refused_create_stays_refused_on_the_host() {
    assert_eq!(
        crate::peer_policy::mode_refusal(SessionKind::Claude, "acceptEdits"),
        Some(crate::peer_policy::PROMPT_SKIPPING_REFUSED),
        "the host's gate refuses a prompt-skipping create"
    );
    let harness = Harness::start("peer-link-operate-mode");
    harness.watch();
    eventually("the link comes up", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Online)
    });
    match harness.links.operate_create(
        "b",
        Some("far-workspace".to_string()),
        SessionKind::Claude,
        Some("claude".to_string()),
        Some("acceptEdits".to_string()),
        None,
        None,
        None,
        None,
    ) {
        LinkAnswer::Created(_) => {}
        other => panic!("the link carries the create; the host decides, got {other:?}"),
    }
    match harness.requests.recv_timeout(Duration::from_secs(5)) {
        Ok(ClientMessage::SessionCreate { kind, mode, .. }) => {
            assert_eq!(kind, SessionKind::Claude);
            assert_eq!(mode.as_deref(), Some("acceptEdits"));
        }
        Ok(other) => panic!("expected the peer's SessionCreate, got {other:?}"),
        Err(error) => panic!("the create never reached the peer: {error:?}"),
    }
}
