//! What one watching connection is told about a host's state. The link's own
//! transitions are in `peer_link_lifecycle_tests`; this is the view a single
//! lease has of them, and the promise a new lease is given when it joins a
//! link that is already up.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use devboule_protocol::RemoteHostState;

use super::harness::Harness;

/// A watch that joins a link already up is answered with the state the link is
/// in now, and moves nothing: the lease that was there first is told nothing,
/// because nothing changed for it.
#[test]
fn a_new_watcher_is_told_the_current_state_and_nothing_else_moves() {
    let harness = Harness::start("peer-link-rewatch");
    harness.watch();
    harness.wait_online(&harness.conn);
    harness.statuses();

    let second = harness.other_conn();
    harness
        .links
        .watch(&harness.state, Arc::clone(&second), "b")
        .expect("the link is already open, so this watch joins it");

    assert_eq!(
        harness.statuses_for(&second),
        vec![(RemoteHostState::Online, None)],
        "a watcher that joined a live link is told the state it cannot have seen"
    );
    std::thread::sleep(Duration::from_millis(200));
    let after = harness.statuses();
    assert!(
        after.is_empty(),
        "the link's state did not change, so the first watcher is told nothing: {after:?}"
    );
    assert_eq!(
        harness.responder.handshakes.load(Ordering::SeqCst),
        1,
        "one watch more is one lease more, not one dial more"
    );
}

/// Online is the union of both directions. A daemon that watches a host holds
/// an outbound link and no inbound connection, and the Devices panel must
/// still say online; an accepted inbound connection alone must too, and a link
/// that published a failure is not online.
#[test]
fn a_peer_is_online_over_either_direction() {
    let harness = Harness::start("peer-online-union");
    assert!(
        !harness.state.is_peer_online("b"),
        "nothing is connected yet"
    );

    harness
        .state
        .peer_links
        .publish_for_test("b", RemoteHostState::Online);
    assert!(
        harness.state.is_peer_online("b"),
        "an outbound link alone is a live connection"
    );
    harness
        .state
        .peer_links
        .publish_for_test("b", RemoteHostState::Offline);
    assert!(
        !harness.state.is_peer_online("b"),
        "a link that published a failure is not online"
    );

    let close = harness.state.register_remote_conn(77, "b");
    assert!(
        harness.state.is_peer_online("b"),
        "an inbound connection alone is a live connection"
    );
    close.store(true, Ordering::SeqCst);
    harness.state.unregister_remote_conn(77);
    assert!(
        !harness.state.is_peer_online("b"),
        "a closed connection is not online"
    );
}

/// A host that pushes a workspace revision is seen by its watchers: the link
/// records the number and re-publishes a status carrying it, which is what
/// tells the sidebar to reload that host's snapshots.
#[test]
fn a_pushed_workspace_revision_reaches_the_watcher() {
    let harness = Harness::start("peer-link-workspace-revision");
    harness.watch();
    harness.wait_online(&harness.conn);

    harness.responder.push_workspace_changed(7);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if harness
            .statuses_with_revision()
            .iter()
            .any(|(state, revision)| *state == RemoteHostState::Online && *revision == Some(7))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the pushed revision never reached the watcher"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // A push naming another device is dropped: the link's own device id is the
    // only identity this side trusts.
    harness
        .responder
        .push_workspace_changed_from("not-b", 99);
    let quiet = Instant::now() + Duration::from_millis(400);
    while Instant::now() < quiet {
        assert!(
            !harness
                .statuses_with_revision()
                .iter()
                .any(|(_, revision)| *revision == Some(99)),
            "a frame for another device must not move this link's revision"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
