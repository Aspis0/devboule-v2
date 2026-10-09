//! The held link's lifecycle: one handshake serving many reads, leases and
//! the grace that closes a link, the caps that refuse before anything opens,
//! the keepalive that closes a silent host, and the revocation that stops it
//! retrying. The remote's own gate policy is in `peer_link_policy_tests`.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use devboule_protocol::{
    ClientMessage, DaemonMessage, RemoteHostList, RemoteHostListBody, RemoteHostState,
};

use super::harness::Harness;
use super::peer_link_test_support::{current_capabilities, eventually, host_row, pinned_keypair};
use super::MAX_HELD_LINKS;
use crate::server::peer_link_state::LinkAnswer;

/// One handshake, several exchanges: the point of holding a link at all.
#[test]
fn several_reads_share_one_handshake() {
    let harness = Harness::start("peer-link-held");
    harness.watch();
    eventually("the link comes up", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Online)
    });
    for _ in 0..3 {
        match harness.links.read("b", RemoteHostList::Projects) {
            LinkAnswer::Body(RemoteHostListBody::Projects { rows }) => {
                assert_eq!(rows.len(), 1, "the remote's own row comes through");
            }
            other => panic!("expected the remote's projects, got {other:?}"),
        }
    }
    assert_eq!(
        harness.responder.handshakes.load(Ordering::SeqCst),
        1,
        "three reads, one handshake: a held link does not redial per read"
    );
}

/// A reply that belongs to another request is dropped, not returned: after a
/// reconnect the far side is one link behind, and a caller must never be handed
/// a stranger's rows.
#[test]
fn a_reply_for_another_request_is_dropped() {
    let harness = Harness::start("peer-link-wrong-id");
    harness.watch();
    eventually("the link comes up", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Online)
    });
    harness.responder.answer_with_wrong_id();
    match harness.links.read("b", RemoteHostList::Projects) {
        LinkAnswer::Body(RemoteHostListBody::Projects { rows }) => assert_eq!(
            rows[0].id, "far-project",
            "the answer is the one that matched this read's id"
        ),
        other => panic!("expected the remote's projects, got {other:?}"),
    }
    assert_eq!(
        harness.requests.recv().expect("the request arrived"),
        ClientMessage::ProjectsList { id: 1 },
        "the wrong-id frame did not cost a second request"
    );
}

/// A read queued while the worker is still inside its handshake belongs to the
/// link that has just been replaced, and is refused rather than answered.
#[test]
fn a_read_queued_during_a_reconnect_is_not_answered_by_the_new_link() {
    let harness = Harness::start("peer-link-stale");
    harness.responder.hold_hello(Duration::from_secs(2));
    harness.watch();
    eventually("the worker is inside its handshake", || {
        harness.responder.hellos.load(Ordering::SeqCst) >= 1
    });
    match harness.links.read("b", RemoteHostList::Sessions) {
        LinkAnswer::Failed(state, sentence) => assert_eq!(
            state,
            RemoteHostState::Offline,
            "a read from the replaced link is refused, not served: {sentence}"
        ),
        other => panic!("expected the stale read to be refused, got {other:?}"),
    }
}

/// The link is closed on the far side: the read in flight comes back on the
/// close rather than on its own ten-second deadline.
#[test]
fn closing_the_far_end_unblocks_the_read() {
    let harness = Harness::with_fast_probes("peer-link-close");
    harness.watch();
    eventually("the link comes up", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Online)
    });
    harness
        .responder
        .answers_pings
        .store(false, Ordering::SeqCst);
    harness.responder.stop();
    let started = std::time::Instant::now();
    match harness.links.read("b", RemoteHostList::Sessions) {
        LinkAnswer::Body(_)
        | LinkAnswer::Refused(_)
        | LinkAnswer::Accepted
        | LinkAnswer::Created(_)
        | LinkAnswer::Sent(_)
        | LinkAnswer::Providers { .. } => {}
        LinkAnswer::Failed(state, sentence) => {
            assert_eq!(state, RemoteHostState::Offline, "{sentence}")
        }
    }
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "a closed link must answer its reader, not wait the ten-second deadline"
    );
    eventually("the link reports itself offline", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Offline)
    });
}

/// Two watches of one host share the link; one lease going is not a close; a
/// dropped connection releases its own leases; the last lease plus the grace
/// closes the link.
#[test]
fn leases_share_a_link_and_the_last_one_closes_it() {
    let harness = Harness::start("peer-link-leases");
    let other = harness.other_conn();
    harness.watch();
    harness
        .links
        .watch(&harness.state, Arc::clone(&other), "b")
        .expect("a second watch shares the link the first opened");
    eventually("the link comes up", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Online)
    });
    assert_eq!(
        harness.responder.handshakes.load(Ordering::SeqCst),
        1,
        "two leases, one link"
    );
    harness.links.unwatch(other.id, "b");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        harness.responder.handshakes.load(Ordering::SeqCst),
        1,
        "one lease left means the link stays"
    );
    // A dropped window releases whatever leases it held, with no unwatch needed.
    harness
        .links
        .watch(&harness.state, Arc::clone(&other), "b")
        .expect("the second window watches again");
    harness.links.release_connection(other.id);
    harness.links.unwatch(harness.conn.id, "b");
    eventually(
        "the link closes once the last lease and its grace are gone",
        || harness.responder.closes.load(Ordering::SeqCst) >= 1,
    );
}

/// A fifth host is refused `busy` with nothing opened.
#[test]
fn a_fifth_host_is_refused_busy_and_nothing_is_opened() {
    let harness = Harness::start("peer-link-cap");
    let keypair = pinned_keypair();
    let mut held = Vec::new();
    for index in 0..MAX_HELD_LINKS {
        let device_id = format!("host-{index}");
        let (responder, _requests) = super::peer_link_test_support::spawn(
            keypair.private.clone().try_into().expect("32 bytes"),
            current_capabilities(),
        );
        let mut row = host_row(responder.address.to_string(), &keypair.public);
        row.device_id = device_id.clone();
        harness.state.peer_upsert(row).expect("upsert the row");
        harness
            .links
            .watch(&harness.state, Arc::clone(&harness.conn), &device_id)
            .expect("four links are inside the cap");
        held.push(responder);
    }
    let (extra_responder, _extra_requests) = super::peer_link_test_support::spawn(
        keypair.private.clone().try_into().expect("32 bytes"),
        current_capabilities(),
    );
    let mut row = host_row(extra_responder.address.to_string(), &keypair.public);
    row.device_id = "host-extra".to_string();
    harness.state.peer_upsert(row).expect("upsert the row");
    assert!(
        harness
            .links
            .watch(&harness.state, Arc::clone(&harness.conn), "host-extra")
            .is_err(),
        "the fifth host is past the cap"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        extra_responder.handshakes.load(Ordering::SeqCst),
        0,
        "a refused watch must not open a socket"
    );
}

/// Two unanswered probes close the link, and the close is one event.
#[test]
fn two_missed_pongs_close_the_link_once() {
    let harness = Harness::with_fast_probes("peer-link-keepalive");
    harness
        .responder
        .answers_pings
        .store(false, Ordering::SeqCst);
    harness.watch();
    // The close is observed where the app sees it: one `offline` edge, and no
    // more of them however long the worker keeps retrying.
    let mut offline = 0;
    eventually("two missed probes close the link", || {
        harness.count_offline(&mut offline);
        offline >= 1
    });
    std::thread::sleep(Duration::from_millis(300));
    let after = harness.statuses();
    offline += after
        .iter()
        .filter(|(state, _)| *state == RemoteHostState::Offline)
        .count();
    assert_eq!(offline, 1, "one offline edge per close, not a flood");
    assert!(
        !after
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Connecting),
        "a retry that still fails is not news the app needs: {after:?}"
    );
}

/// A revoked row closes the link, publishes `needs_pairing`, and opens no
/// further socket: the retry path re-reads the row and stops at it.
#[test]
fn a_revoked_row_closes_the_link_and_is_never_retried() {
    let harness = Harness::start("peer-link-revoke");
    harness.watch();
    eventually("the link comes up", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Online)
    });
    assert_eq!(harness.responder.handshakes.load(Ordering::SeqCst), 1);
    harness.state.peer_revoke("b", 1).expect("revoke the row");
    eventually("the revocation is heard", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::NeedsPairing)
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        harness.responder.handshakes.load(Ordering::SeqCst),
        1,
        "a revoked peer is never dialled again"
    );
    match harness.links.read("b", RemoteHostList::Sessions) {
        LinkAnswer::Failed(state, _) => assert_eq!(
            state,
            RemoteHostState::NeedsPairing,
            "the read carries the same repair the row does"
        ),
        other => panic!("expected needs_pairing, got {other:?}"),
    }
}

/// One connection's watch, list and status push, end to end through the router
/// the connection handler calls. The handle carries a kernel identity like a
/// real app pipe client: the app-only door refuses identity-less handles.
#[test]
fn the_router_answers_a_watch_a_list_and_an_unwatch() {
    let harness = Harness::start("peer-link-router");
    let conn = crate::session::ConnHandle::with_peer(
        harness.state.alloc_conn(),
        Some(crate::agent_report::PeerIdentity {
            user: "S-test-user".to_string(),
            pid: 1,
        }),
    );
    conn.set_remote_hosts_negotiated(true);
    let watched = crate::server::peer_link_dispatch::dispatch_remote_host(
        &harness.state,
        &conn,
        ClientMessage::RemoteHostWatch {
            id: 1,
            device_id: "b".to_string(),
        },
    );
    assert!(
        matches!(watched, DaemonMessage::Ok { id: 1 }),
        "the lease is held: {watched:?}"
    );
    harness.wait_online(&conn);
    match crate::server::peer_link_dispatch::dispatch_remote_host(
        &harness.state,
        &conn,
        ClientMessage::RemoteHostList {
            id: 2,
            device_id: "b".to_string(),
            list: RemoteHostList::Sessions,
        },
    ) {
        DaemonMessage::RemoteHostList { id, body, .. } => {
            assert_eq!(id, 2);
            assert!(
                matches!(body, RemoteHostListBody::Sessions { .. }),
                "the remote's own body comes back: {body:?}"
            );
        }
        other => panic!(
            "expected the remote's sessions, got {other:?}; statuses so far: {:?}",
            harness.statuses_for(&conn)
        ),
    }
    let unwatched = crate::server::peer_link_dispatch::dispatch_remote_host(
        &harness.state,
        &conn,
        ClientMessage::RemoteHostUnwatch {
            id: 3,
            device_id: "b".to_string(),
        },
    );
    assert!(
        matches!(unwatched, DaemonMessage::Ok { id: 3 }),
        "{unwatched:?}"
    );
    // Unwatching a host nobody watches is a no-op, not an error.
    let again = crate::server::peer_link_dispatch::dispatch_remote_host(
        &harness.state,
        &conn,
        ClientMessage::RemoteHostUnwatch {
            id: 4,
            device_id: "b".to_string(),
        },
    );
    assert!(matches!(again, DaemonMessage::Ok { id: 4 }), "{again:?}");
}

/// A read past the one-in-flight bound is refused `busy` rather than queued
/// behind a link that may never come back.
#[test]
fn a_second_read_on_one_link_is_refused_busy() {
    let harness = Harness::start("peer-link-busy-read");
    harness.responder.hold_reads();
    harness.watch();
    eventually("the link comes up", || {
        harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Online)
    });
    // The far end takes the read and stays quiet, so the first read parks in the
    // worker; the second must be refused rather than queued behind it.
    std::thread::scope(|scope| {
        let first = scope.spawn(|| harness.links.read("b", RemoteHostList::Sessions));
        std::thread::sleep(Duration::from_millis(100));
        match harness.links.read("b", RemoteHostList::Sessions) {
            LinkAnswer::Failed(RemoteHostState::Busy, sentence) => {
                assert!(sentence.contains("read in flight"), "{sentence}");
            }
            other => panic!("expected the read budget, got {other:?}"),
        }
        let _ = first.join();
    });
}
