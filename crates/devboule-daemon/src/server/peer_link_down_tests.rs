//! A read on a link that is not up: the host the daemon already knows is down
//! answers at once, not when the caller's own budget runs out.

use std::sync::mpsc;
use std::time::Duration;

use devboule_protocol::{RemoteHostList, RemoteHostState};

use super::harness::Harness;
use super::peer_link_test_support::eventually;
use crate::server::peer_dial::DialStep;
use crate::server::peer_link_state::{LinkAnswer, LinkCommand};
use crate::server::peer_link_worker::refuse_reads_until;

/// A read that was already queued when the failure was published is answered by
/// the worker's own wait, not by the caller's check: the backoff is four
/// seconds and the answer comes inside two.
#[test]
fn a_read_queued_before_the_failure_is_published_is_answered_during_the_backoff() {
    let (commands, queue) = mpsc::sync_channel(1);
    let (answer, answers) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        scope.spawn(move || {
            refuse_reads_until(&queue, DialStep::Connect, Duration::from_secs(4));
        });
        commands
            .try_send(LinkCommand::Read {
                generation: 0,
                list: RemoteHostList::Sessions,
                answer,
            })
            .expect("the queue has room");
        match answers.recv_timeout(Duration::from_secs(2)) {
            Ok(LinkAnswer::Failed(state, _)) => assert_eq!(state, RemoteHostState::Offline),
            other => panic!("expected the dial's failure inside the backoff, got {other:?}"),
        }
    });
}

/// The link has published its failure and its worker is asleep in the backoff
/// when the read arrives. The answer is that failure, and it comes while the
/// backoff is still running: the backoff here is longer than the wait for it.
#[test]
fn a_read_during_the_backoff_is_answered_with_the_failure_at_once() {
    let harness = Harness::host_down("peer-link-down-read", Duration::from_secs(8));
    harness.watch();
    let mut failure = None;
    eventually("the failed open is published", || {
        failure = harness
            .statuses()
            .into_iter()
            .find(|(state, _)| *state == RemoteHostState::Offline)
            .and_then(|(_, last_failure)| last_failure);
        failure.is_some()
    });
    let (answered, answers) = mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = answered.send(harness.links.read("b", RemoteHostList::Sessions));
        });
        match answers.recv_timeout(Duration::from_secs(3)) {
            Ok(LinkAnswer::Failed(state, sentence)) => {
                assert_eq!(state, RemoteHostState::Offline);
                assert_eq!(
                    Some(sentence),
                    failure,
                    "the answer carries the failure the link published"
                );
            }
            Ok(other) => panic!("expected the link's failure, got {other:?}"),
            Err(_) => panic!("the read was not answered while the link was in backoff"),
        }
    });
}
