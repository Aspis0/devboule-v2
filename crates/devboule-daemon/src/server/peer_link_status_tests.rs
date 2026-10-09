//! What one watching connection is told about a host's state. The link's own
//! transitions are in `peer_link_lifecycle_tests`; this is the view a single
//! lease has of them, and the promise a new lease is given when it joins a
//! link that is already up.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use devboule_protocol::{RemoteHostState, WorkspaceIsolation};

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

    let close = harness
        .state
        .register_remote_conn(remote_conn_handle(77, "b"));
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
    harness.responder.push_workspace_changed_from("not-b", 99);
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

/// One connection that looks like an accepted peer to the state: its peer
/// identity is what the registry records and the push addresses.
fn remote_conn_handle(id: u64, device_id: &str) -> std::sync::Arc<crate::session::ConnHandle> {
    crate::session::ConnHandle::with_conn_peer(
        id,
        None,
        Some(crate::peer_policy::ConnPeer::Remote {
            device_id: device_id.to_string(),
            scope: crate::peer_policy::PeerScope::PeerDevice,
            paired_by_user: None,
            binding: crate::peer_policy::TransportBinding::tailnet("nstable", "node", "user@x"),
        }),
    )
}

/// A workspace change is pushed to the peer connections whose dialect knows
/// the frame, and to no others: a v31 connection that cannot decode it is
/// never sent one.
#[test]
fn a_workspace_change_is_pushed_only_to_dialects_that_know_it() {
    let harness = Harness::start("peer-link-workspace-push");
    let negotiating = remote_conn_handle(1, "b");
    negotiating.set_negotiated_protocol(32);
    harness.state.register_remote_conn(Arc::clone(&negotiating));
    let plain = remote_conn_handle(2, "c");
    plain.set_negotiated_protocol(31);
    harness.state.register_remote_conn(Arc::clone(&plain));

    let revision = harness.state.note_workspace_change();
    assert_eq!(
        revision, 2,
        "the first change moves the revision off its seed"
    );

    let pushed: Vec<(String, u64)> = negotiating
        .outbound
        .pull_replies()
        .into_iter()
        .filter_map(|reply| match reply {
            devboule_protocol::DaemonMessage::HostWorkspaceChanged {
                device_id,
                revision,
            } => Some((device_id, revision)),
            _ => None,
        })
        .collect();
    assert_eq!(pushed.len(), 1, "one revision, one push");
    assert_eq!(pushed[0].1, revision);
    assert_eq!(
        pushed[0].0,
        harness
            .state
            .device_identity()
            .as_ref()
            .expect("identity")
            .device_id,
        "the push names this daemon, the only host the connection is about"
    );
    assert!(
        !plain.outbound.pull_replies().iter().any(|reply| matches!(
            reply,
            devboule_protocol::DaemonMessage::HostWorkspaceChanged { .. }
        )),
        "a v31 connection is never sent the frame it cannot decode"
    );
}

/// The reconnect flag is one-shot: a worker redials once per request and a
/// taken request is not replayed on the next loop.
#[test]
fn a_reconnect_request_is_taken_once() {
    let link = super::HostLink::new("b".to_string(), Duration::from_secs(1));
    assert!(!link.take_reconnect_request());
    link.request_reconnect();
    assert!(link.take_reconnect_request());
    assert!(
        !link.take_reconnect_request(),
        "a taken request must not redial twice"
    );
}

/// The first workspace flips this daemon into a host: every held link is asked
/// to redial, so the hello it sends states the new presence and the far side
/// re-resolves the scope from its own record.
#[test]
fn the_first_workspace_asks_links_to_redial() {
    let harness = Harness::start("peer-link-presence-flip");
    // The state's own link manager this time: the transition hook fans out to
    // `state.peer_links`, which is where a production daemon keeps its links.
    harness
        .state
        .peer_links
        .watch(&harness.state, std::sync::Arc::clone(&harness.conn), "b")
        .expect("the first watch is inside the link cap");
    harness.wait_online(&harness.conn);
    assert!(!harness.state.has_hosted_workspace());

    let dir = crate::test_dirs::test_temp_dir("devboule presence flip");
    let project_dir = dir.join("project");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    let project = harness
        .state
        .sessions
        .project_add(project_dir.to_str().expect("utf-8 path"))
        .expect("project row");
    harness
        .state
        .sessions
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("workspace row");
    assert!(harness.state.has_hosted_workspace());

    // The caller captured the hosting state before the mutation; the hook sees
    // the flip and asks the link to redial. The test responder served its one
    // connection already, so the redial finds nothing and the link reports
    // offline — which is the observable proof the transport was replaced.
    harness.state.note_workspace_inventory(false);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if harness
            .statuses()
            .iter()
            .any(|(state, _)| *state == RemoteHostState::Offline)
        {
            break;
        }
        assert!(Instant::now() < deadline, "the flipped link never redialed");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        harness.responder.handshakes.load(Ordering::SeqCst),
        1,
        "the redial found the responder's listener gone"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
