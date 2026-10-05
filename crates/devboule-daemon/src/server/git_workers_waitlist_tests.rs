//! Waitlist admission and answers under a controlled worker: no git and
//! no clock-driven waits — every compute reports its id and blocks on the
//! test's own release channel, so the drain's order comes off events the
//! test itself causes; the receive bounds are hang-catchers. Red either
//! way the policy could break: a parked read never admitted never starts
//! (a receive below times out), and an admitted read whose sink never
//! fires leaves the answer count short.

use std::sync::mpsc;
use std::time::Duration;

use devboule_protocol::DaemonMessage;

use super::super::git_queue::{GitQueue, Job, ReadKey, Sink};
use super::*;

/// Ten distinct-path reads at the caps — four queued, five parked — each
/// answered exactly once, with the freed slots admitting the waitlist
/// newest-first: the read the user just asked for is the one whose latency
/// they feel, and the older parked reads answer after it, in turn, none
/// dropped.
#[test]
fn every_parked_read_is_answered_and_each_slot_admits_the_newest() {
    let queue = GitQueue::default();
    let root = "waitlist-controlled".to_string();
    let (started_tx, started_rx) = mpsc::channel::<u64>();
    let (answered_tx, answered_rx) = mpsc::channel::<u64>();
    let mut releases = Vec::new();
    let mut jobs = Vec::new();
    for id in 1..=10u64 {
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        releases.push(release_tx);
        let started = started_tx.clone();
        let answered = answered_tx.clone();
        jobs.push(Job::Read {
            key: ReadKey::new(90, Some(format!("f{id}.txt"))),
            compute: Box::new(move || {
                let _ = started.send(id);
                let _ = release_rx.recv_timeout(Duration::from_secs(30));
                DaemonMessage::Error(WireError::new(ErrorCode::Io, "the controlled read"))
            }),
            sinks: Arc::new(Mutex::new(vec![Sink::new(Box::new(move |_reply| {
                let _ = answered.send(id);
            }))])),
        });
    }
    let mut jobs = jobs.into_iter();
    assert!(queue
        .enqueue_job(root.clone(), jobs.next().expect("the first job"))
        .is_ok());
    assert_eq!(
        started_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("the drain must start the armed read"),
        1
    );
    for job in jobs {
        assert!(queue.enqueue_job(root.clone(), job).is_ok());
    }
    assert_eq!(
        queue.queue_shape(&root),
        (4, 4, 5),
        "the burst parks at the caps: four queued reads, five waitlisted"
    );
    // Each release frees exactly one slot and the serial drain refills it
    // from the waitlist's back, so the sequence below is the drain's own
    // order, read off the started channel rather than off a clock.
    let mut running = 1u64;
    for expected in [2u64, 3, 4, 5, 10, 9, 8, 7, 6] {
        releases[(running - 1) as usize]
            .send(())
            .expect("release the running read");
        running = started_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("a parked read must start when a slot frees");
        assert_eq!(
            running, expected,
            "freed slots admit the waitlist newest first"
        );
    }
    assert_eq!(
        queue.queue_shape(&root),
        (0, 0, 0),
        "every parked read was admitted; nothing is left waiting"
    );
    releases[(running - 1) as usize]
        .send(())
        .expect("release the last read");
    let mut answered = Vec::new();
    for _ in 0..10 {
        answered.push(
            answered_rx
                .recv_timeout(Duration::from_secs(30))
                .expect("every admitted read answers its own caller"),
        );
    }
    assert_eq!(
        answered,
        vec![1, 2, 3, 4, 5, 10, 9, 8, 7, 6],
        "every request is answered, in admission order"
    );
}
