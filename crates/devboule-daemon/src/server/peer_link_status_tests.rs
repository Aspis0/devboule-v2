//! What one watching connection is told about a host's state. The link's own
//! transitions are in `peer_link_lifecycle_tests`; this is the view a single
//! lease has of them, and the promise a new lease is given when it joins a
//! link that is already up.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

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
