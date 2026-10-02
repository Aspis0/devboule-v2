//! What a paired device may do to a queue: a device that may send into a
//! session may queue there and may press send-now, a device that may only
//! watch may do neither, and a connection that never negotiated the queue
//! capability is refused at the door.

use super::session_queue::queue_items_for_test;
use super::session_queue_fixtures::{attached, queue_registry, queued_agent};
use super::tests::{remote_conn, test_owner};
use super::*;
use crate::peer_policy::{peer_allows, PeerDecision, CAP_SEND, CAP_VIEW};
use devboule_protocol::ClientMessage;

/// The decision the peer gate makes for one frame from a device holding
/// `caps`, with the audit side of the gate left out.
fn peer_decision(caps: &[&str], request: &ClientMessage) -> PeerDecision {
    peer_allows(
        PeerRole::Client,
        &caps
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>(),
        request,
    )
}

fn add_frame() -> ClientMessage {
    ClientMessage::SessionQueueAdd {
        id: 1,
        session_id: "s.a.1".to_string(),
        client_operation_id: "op-1".to_string(),
        text: "from a phone".to_string(),
        attachments: Vec::new(),
        attachment_references: Vec::new(),
    }
}

fn send_now_frame() -> ClientMessage {
    ClientMessage::SessionQueueSendNow {
        id: 1,
        session_id: "s.a.1".to_string(),
        client_operation_id: "op-1".to_string(),
        subscription_id: 1,
        item_id: "queue-1".to_string(),
    }
}

#[test]
fn the_four_edits_ride_the_capability_a_send_rides() {
    for frame in [
        add_frame(),
        ClientMessage::SessionQueueEdit {
            id: 1,
            session_id: "s.a.1".to_string(),
            client_operation_id: "op-1".to_string(),
            item_id: "queue-1".to_string(),
            text: "edited".to_string(),
        },
        ClientMessage::SessionQueueRemove {
            id: 1,
            session_id: "s.a.1".to_string(),
            client_operation_id: "op-1".to_string(),
            item_id: "queue-1".to_string(),
        },
        ClientMessage::SessionQueueMove {
            id: 1,
            session_id: "s.a.1".to_string(),
            client_operation_id: "op-1".to_string(),
            item_id: "queue-1".to_string(),
            to_index: 0,
        },
    ] {
        assert_eq!(
            peer_decision(&[CAP_VIEW, CAP_SEND], &frame),
            PeerDecision::Allow,
            "{} must open to a device that may send",
            frame.name()
        );
        assert_eq!(
            peer_decision(&[CAP_VIEW], &frame),
            PeerDecision::Deny(CAP_SEND),
            "{} must refuse a device that may only watch",
            frame.name()
        );
    }
}

/// Send-now is the same act as the interrupting send, so it rides the same
/// capability. What stops a paired device from actually interrupting is the
/// send path, which refuses an interrupt from a peer wherever it is asked for
/// one — the rule `send_with_subscription_behavior` applies to a direct send.
#[test]
fn send_now_rides_the_capability_a_send_rides() {
    assert_eq!(
        peer_decision(&[CAP_VIEW, CAP_SEND], &send_now_frame()),
        PeerDecision::Allow,
        "a device that may send into this session may say what it says next"
    );
    assert_eq!(
        peer_decision(&[CAP_VIEW], &send_now_frame()),
        PeerDecision::Deny(CAP_SEND)
    );
}

#[test]
fn a_device_that_may_send_may_queue_into_the_session_it_may_send_to() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-peer", "process-peer");
    let id = compose_session_id(&owner.session_token(), "p").expect("id");
    let (runtime, _sent) = queued_agent(&registry, &journal, &owner, &id);
    runtime.begin_turn();
    let phone = remote_conn(PeerRole::Client, Some(owner.user.as_str()));

    registry
        .queue_add(&id, "op-1", "from a phone", &[], &[], &owner, &phone)
        .expect("a send-capable device may queue");

    assert_eq!(queue_items_for_test(&registry, &id).len(), 1);

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_device_that_may_not_reach_the_session_may_not_queue_into_it() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-peer2", "process-peer2");
    let id = compose_session_id(&owner.session_token(), "p").expect("id");
    let (runtime, _sent) = queued_agent(&registry, &journal, &owner, &id);
    runtime.begin_turn();
    // A device another account paired: its scope never opens this session.
    let stranger = remote_conn(PeerRole::Client, Some("S-1-5-21-someone-else"));

    let error = registry
        .queue_add(&id, "op-1", "not yours", &[], &[], &owner, &stranger)
        .expect_err("the session belongs to another user");

    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert!(queue_items_for_test(&registry, &id).is_empty());

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_snapshot_reaches_only_connections_attached_to_the_session() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-vis", "process-vis");
    let id = compose_session_id(&owner.session_token(), "v").expect("id");
    let (runtime, _sent) = queued_agent(&registry, &journal, &owner, &id);
    runtime.begin_turn();
    let attached_conn = attached(&registry, &id, 4, &owner);
    let _ = attached_conn.pull_events();
    // A second connection that never attached: it is on the daemon, and it
    // holds a subscription to nothing.
    let bystander = ConnHandle::new(9);

    registry
        .queue_add(
            &id,
            "op-1",
            "only for the attached",
            &[],
            &[],
            &owner,
            &attached_conn,
        )
        .expect("add");

    assert_eq!(
        super::session_queue_fixtures::snapshots(&attached_conn).len(),
        1,
        "the attached client sees it"
    );
    assert!(
        bystander.pull_events().is_empty(),
        "a connection that is not attached sees nothing"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
