//! What one watching connection is told about a host's state. The link's own
//! transitions are in `peer_link_lifecycle_tests`; this is the view a single
//! lease has of them, and the promise a new lease is given when it joins a
//! link that is already up.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use devboule_protocol::{
    ClientMessage, DaemonMessage, RemoteHostState, RemoteHostStatus, WorkspaceIsolation,
};

use super::harness::Harness;
use super::peer_link_test_support::eventually;

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

    // The next number in the sequence moves it.
    harness.responder.push_workspace_changed(8);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if harness
            .statuses_with_revision()
            .iter()
            .any(|(_, revision)| *revision == Some(8))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a continuing revision never reached the watcher"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // A jump over an unseen value, a replay, and a frame for another device
    // are all refused: none may move the link's revision or produce a push.
    harness.responder.push_workspace_changed(50);
    harness.responder.push_workspace_changed(8);
    harness.responder.push_workspace_changed_from("not-b", 99);
    let quiet = Instant::now() + Duration::from_millis(400);
    while Instant::now() < quiet {
        let revisions = harness.statuses_with_revision();
        assert!(
            !revisions
                .iter()
                .any(|(_, revision)| matches!(revision, Some(50) | Some(99))),
            "a jump or a foreign device moved this link's revision: {revisions:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The battery rule at the link: a burst of a hundred distinct statuses
/// delivers one immediately and exactly one trailing value — the last — to
/// every watcher, so a flapping host wakes them twice, not a hundred times,
/// and the final state is still the one delivered last.
#[test]
fn a_status_burst_delivers_one_immediate_and_one_trailing_value() {
    fn statuses(conn: &Arc<crate::session::ConnHandle>) -> Vec<RemoteHostStatus> {
        conn.outbound
            .pull_replies()
            .into_iter()
            .filter_map(|reply| match reply {
                DaemonMessage::RemoteHostStatus {
                    device_id,
                    state,
                    last_failure,
                    revision,
                } => Some(RemoteHostStatus {
                    device_id,
                    state,
                    last_failure,
                    revision,
                }),
                _ => None,
            })
            .collect()
    }

    let link = super::HostLink::new(
        "b".to_string(),
        Duration::from_secs(1),
        Duration::from_secs(1),
    );
    let conn = remote_conn_handle(3, "b");
    link.lease(Arc::clone(&conn));
    let start = Instant::now();
    for value in 0..100 {
        link.publish(RemoteHostStatus {
            device_id: "b".to_string(),
            state: RemoteHostState::Online,
            last_failure: Some(value.to_string()),
            revision: None,
        });
    }

    let immediate = statuses(&conn);
    assert_eq!(immediate.len(), 1, "the burst delivers one status now");
    assert_eq!(immediate[0].last_failure.as_deref(), Some("0"));

    link.flush_status(start + Duration::from_millis(500));
    assert!(
        statuses(&conn).is_empty(),
        "the trailing value waits out the window"
    );
    link.flush_status(start + Duration::from_millis(1100));
    let trailing = statuses(&conn);
    assert_eq!(trailing.len(), 1, "one trailing delivery, not ninety-nine");
    assert_eq!(
        trailing[0].last_failure.as_deref(),
        Some("99"),
        "the trailing value is the final state"
    );
}

/// One relayed event carries the host it came from, the remote session and
/// the local subscription the app opened, in one frame the app can route
/// without guessing: two machines may mint the same session id.
#[test]
fn an_attach_relays_session_events_with_the_host_and_subscription() {
    let harness = Harness::start("peer-link-attach");
    harness.watch();
    harness.wait_online(&harness.conn);

    let answer = harness
        .links
        .attach("b", "session-1", 7, Arc::clone(&harness.conn));
    assert!(
        matches!(answer, super::LinkAnswer::Accepted),
        "the attach is accepted: {answer:?}"
    );

    // The replayed event the fake peer sends right after the attach.
    let mut events = Vec::new();
    eventually("the replayed event reaches the app", || {
        events = harness
            .conn
            .outbound
            .pull_replies()
            .into_iter()
            .filter_map(|reply| match reply {
                DaemonMessage::RemoteHostEvent {
                    device_id,
                    session_id,
                    subscription_id,
                    envelope,
                } => Some((device_id, session_id, subscription_id, envelope)),
                _ => None,
            })
            .collect();
        events
            .iter()
            .any(|(_, _, subscription_id, _)| *subscription_id == 7)
    });
    let (device_id, session_id, subscription_id, envelope) = &events[0];
    assert_eq!(device_id, "b");
    assert_eq!(session_id, "session-1");
    assert_eq!(*subscription_id, 7);
    match &envelope.event {
        devboule_protocol::SessionEvent::Output { data, .. } => assert_eq!(data, "replayed"),
        other => panic!("expected the replayed output, got {other:?}"),
    }

    // A later push on the same subscription relays too.
    harness.responder.push_session_event(7, "later");
    eventually("a pushed event relays", || {
        harness
            .conn
            .outbound
            .pull_replies()
            .into_iter()
            .any(|reply| {
                matches!(
                    reply,
                    DaemonMessage::RemoteHostEvent {
                        subscription_id: 7,
                        ..
                    }
                )
            })
    });
}

/// A detach removes the local subscription first, so nothing is relayed after
/// the app asked to stop.
#[test]
fn a_detach_stops_the_relay() {
    let harness = Harness::start("peer-link-detach");
    harness.watch();
    harness.wait_online(&harness.conn);

    let answer = harness
        .links
        .attach("b", "session-1", 7, Arc::clone(&harness.conn));
    assert!(matches!(answer, super::LinkAnswer::Accepted));
    eventually("the attach lands", || {
        harness
            .conn
            .outbound
            .pull_replies()
            .into_iter()
            .any(|reply| matches!(reply, DaemonMessage::RemoteHostEvent { .. }))
    });

    let answer = harness.links.detach("b", "session-1", 7);
    assert!(matches!(answer, super::LinkAnswer::Accepted));
    harness.responder.push_session_event(7, "after-detach");

    let quiet = Instant::now() + Duration::from_millis(400);
    while Instant::now() < quiet {
        assert!(
            !harness
                .conn
                .outbound
                .pull_replies()
                .into_iter()
                .any(|reply| matches!(reply, DaemonMessage::RemoteHostEvent { .. })),
            "an event after a detach must not be relayed"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A detach while the link is busy still closes the stream locally at once —
/// nothing is relayed after it — and the peer is asked to stop at the first
/// free turn instead of being left streaming.
#[test]
fn a_busy_link_still_closes_a_detached_stream_and_stops_the_peer() {
    let harness = Harness::start("peer-link-detach-busy");
    harness.watch();
    harness.wait_online(&harness.conn);
    assert!(matches!(
        harness
            .links
            .attach("b", "session-1", 7, Arc::clone(&harness.conn)),
        super::LinkAnswer::Accepted
    ));
    eventually("the first stream relays", || {
        harness
            .conn
            .outbound
            .pull_replies()
            .into_iter()
            .any(|reply| matches!(reply, DaemonMessage::RemoteHostEvent { .. }))
    });

    // Occupy the link's one command slot with an attach the fake peer never
    // answers; the harness read deadline is five seconds.
    harness.responder.hold_reads();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = harness
                .links
                .attach("b", "session-2", 9, Arc::clone(&harness.conn));
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            matches!(
                harness.links.detach("b", "session-1", 7),
                super::LinkAnswer::Accepted
            ),
            "the local close must not be refused by a busy link"
        );
        // The peer keeps pushing for the detached stream; none of it relays.
        harness.responder.push_session_event(7, "after-detach");
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            !harness
                .conn
                .outbound
                .pull_replies()
                .into_iter()
                .any(|reply| matches!(
                    reply,
                    DaemonMessage::RemoteHostEvent {
                        subscription_id: 7,
                        ..
                    }
                )),
            "a detached stream must not relay, busy link or not"
        );
    });

    // The parked detach goes out once the held attach gives the link back.
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match harness.requests.recv_timeout(Duration::from_millis(100)) {
            Ok(ClientMessage::SessionDetach {
                subscription_id, ..
            }) => {
                assert_eq!(subscription_id, 7, "the detached stream is named");
                break;
            }
            Ok(_) => continue,
            Err(_) if Instant::now() < deadline => continue,
            Err(_) => panic!("the deferred detach never reached the peer"),
        }
    }
}

/// A new transport fences the old generation's subscriptions: the app/// A new transport fences the old generation's subscriptions: the app
/// reattaches on the online edge, and a stale attach never relays into the
/// link that replaced it.
#[test]
fn a_generation_change_drops_the_subscriptions() {
    let link = super::HostLink::new("b".to_string(), Duration::from_secs(1), Duration::ZERO);
    let conn = remote_conn_handle(3, "b");
    let envelope = devboule_protocol::SessionEventEnvelope {
        session_id: "session-1".to_string(),
        generation: 1,
        transcript_seq: None,
        event: devboule_protocol::SessionEvent::Output {
            seq: 1,
            data: "x".to_string(),
        },
    };
    link.register_subscription(7, Arc::clone(&conn), "session-1".to_string());
    assert!(link.forward_event(7, &envelope));
    link.clear_subscriptions();
    assert!(
        !link.forward_event(7, &envelope),
        "a subscription cleared with its generation relays nothing"
    );
    assert!(conn.outbound.pull_replies().len() <= 1);
}

/// The relayed-event queue is bounded: a stopped reader keeps the newest
/// events and drops the oldest, and the link worker never blocks.
#[test]
fn the_relayed_event_queue_drops_the_oldest_at_the_cap() {
    let conn = crate::outbound::ConnOut::new();
    let cap = crate::server::peer_link_state::MAX_RELAYED_EVENTS_PER_CONNECTION;
    for index in 0..cap + 44 {
        conn.enqueue_relayed_event(
            DaemonMessage::RemoteHostEvent {
                device_id: "b".to_string(),
                session_id: "session-1".to_string(),
                subscription_id: 7,
                envelope: devboule_protocol::SessionEventEnvelope {
                    session_id: "session-1".to_string(),
                    generation: 1,
                    transcript_seq: None,
                    event: devboule_protocol::SessionEvent::Output {
                        seq: index as u64,
                        data: index.to_string(),
                    },
                },
            },
            cap,
        );
    }
    let queued: Vec<_> = conn
        .pull_replies()
        .into_iter()
        .filter_map(|reply| match reply {
            DaemonMessage::RemoteHostEvent { envelope, .. } => Some(envelope),
            _ => None,
        })
        .collect();
    assert_eq!(queued.len(), cap, "the queue holds exactly its cap");
    match &queued[0].event {
        devboule_protocol::SessionEvent::Output { data, .. } => {
            assert_eq!(data, "44", "the oldest events were dropped first");
        }
        other => panic!("expected an output, got {other:?}"),
    }
}

/// A revoked peer cannot be attached to: the row is re-read on the way in,
/// exactly as it is before a read.
#[test]
fn an_attach_to_a_revoked_peer_is_refused() {
    let harness = Harness::start("peer-link-attach-revoked");
    harness.watch();
    harness.wait_online(&harness.conn);
    harness.state.peer_revoke("b", 7).expect("revoke");

    let answer = harness
        .links
        .attach("b", "session-1", 7, Arc::clone(&harness.conn));
    match answer {
        super::LinkAnswer::Failed(state, _) => {
            assert_eq!(state, RemoteHostState::NeedsPairing);
        }
        other => panic!("a revoked peer must refuse the attach, got {other:?}"),
    }
}

/// The revision rule itself: a link's first number is its baseline, later ones/// The revision rule itself: a link's first number is its baseline, later ones/// The revision rule itself: a link's first number is its baseline, later ones
/// must continue it, a replay or a poisoned value moves nothing, and a new
/// transport re-baselines.
#[test]
fn a_pushed_revision_must_be_a_continuation() {
    let link = super::HostLink::new("b".to_string(), Duration::from_secs(1), Duration::ZERO);
    assert_eq!(link.remote_revision(), None);
    assert!(
        link.set_remote_revision(2),
        "the first number of a link is its baseline"
    );
    assert!(!link.set_remote_revision(2), "a replay is refused");
    assert!(!link.set_remote_revision(50), "a jump is refused");
    assert!(link.set_remote_revision(3));
    assert_eq!(link.remote_revision(), Some(3));

    link.clear_remote_revision();
    assert_eq!(link.remote_revision(), None);
    assert!(!link.set_remote_revision(0), "zero is never a revision");
    assert!(
        !link.set_remote_revision(u64::MAX),
        "a poisoned u64 is refused as a baseline"
    );
    assert!(link.set_remote_revision(1), "a fresh link re-baselines");
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
    let link = super::HostLink::new("b".to_string(), Duration::from_secs(1), Duration::ZERO);
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
