//! What the remote machine decides about a held-link read: which capability
//! each list needs there, what a refusal looks like when it travels back, and
//! what happens when the far daemon is too old to speak the frame at all.
//!
//! The link never widens a grant: the local daemon asks, the remote's
//! `run_gate` answers, and the answer is carried through as the remote's own.

use devboule_protocol::{
    caps, Capability, ClientMessage, ErrorCode, RemoteHostList, RemoteHostState, WireError,
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
/// third machine, and no capability opens that.
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
