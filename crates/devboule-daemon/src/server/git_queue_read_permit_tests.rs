//! A read reserves its lane permit at pick, like a value job: two roots
//! racing for the last permit never both believe they hold it, and a pick
//! that runs something else takes nothing.

use std::sync::mpsc;
use std::time::Duration;

use super::*;

fn unit_read_job() -> Job {
    Job::Read {
        key: ReadKey::new(7, None),
        compute: Box::new(|| DaemonMessage::Error(WireError::new(ErrorCode::Io, "test read"))),
        sinks: Arc::new(Mutex::new(Vec::new())),
    }
}

fn read_permits(shared: &Shared) -> usize {
    *shared
        .read_permits
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn set_read_permits(shared: &Shared, count: usize) {
    *shared
        .read_permits
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = count;
}

fn live_read_job(
    entered: mpsc::Sender<String>,
    release: mpsc::Receiver<()>,
    done: mpsc::Sender<String>,
    name: &str,
) -> Job {
    let name = name.to_string();
    let reply_name = name.clone();
    Job::Read {
        key: ReadKey::new(1, None),
        compute: Box::new(move || {
            entered
                .send(name.clone())
                .expect("the harness holds the entered receiver");
            release.recv().expect("the harness releases the winner");
            done.send(reply_name.clone())
                .expect("the harness holds the done receiver");
            DaemonMessage::Error(WireError::new(ErrorCode::Io, "test read"))
        }),
        sinks: Arc::new(Mutex::new(Vec::new())),
    }
}

#[test]
fn read_pick_reserves_its_permit() {
    // Two roots share one lane with a single permit left: the first pick
    // takes it, the second finds none and stays queued.
    let shared = Shared::default();
    set_read_permits(&shared, 1);
    let mut first = Inner::default();
    first.jobs.push_back(unit_read_job());
    let mut second = Inner::default();
    second.jobs.push_back(unit_read_job());

    let Pick::Run(_) = next_runnable(&shared, &mut first) else {
        panic!("a read with a free permit is picked");
    };
    assert_eq!(read_permits(&shared), 0, "the pick takes the read permit");
    assert!(first.jobs.is_empty());

    assert!(
        matches!(next_runnable(&shared, &mut second), Pick::Wait(Lane::Read)),
        "without a permit the read waits in its queue"
    );
    assert_eq!(second.jobs.len(), 1, "the loser stays queued");

    // The freed permit lets the loser run: hand back the first pick's
    // permit the way the drain's guard does, then pick again.
    set_read_permits(&shared, 1);
    let Pick::Run(_) = next_runnable(&shared, &mut second) else {
        panic!("the loser runs once the permit frees");
    };
    assert_eq!(read_permits(&shared), 0);
    assert!(second.jobs.is_empty());

    // Live drains: two roots race for the last permit; the loser never
    // blocks in acquire — its job stays queued until the permit frees.
    let queue = GitQueue::default();
    set_read_permits(&queue.shared, 1);
    let (entered_tx, entered_rx) = mpsc::channel::<String>();
    let (done_tx, done_rx) = mpsc::channel::<String>();
    let (release_a_tx, release_a_rx) = mpsc::channel::<()>();
    let (release_b_tx, release_b_rx) = mpsc::channel::<()>();
    queue
        .enqueue_job(
            "read-permit-race-a".to_string(),
            live_read_job(entered_tx.clone(), release_a_rx, done_tx.clone(), "a"),
        )
        .expect("the worker spawns");
    queue
        .enqueue_job(
            "read-permit-race-b".to_string(),
            live_read_job(entered_tx, release_b_rx, done_tx, "b"),
        )
        .expect("the worker spawns");
    let winner = entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("one root's read holds the last permit");
    let (loser, release_loser, release_winner) = if winner == "a" {
        ("read-permit-race-b", release_b_tx, release_a_tx)
    } else {
        ("read-permit-race-a", release_a_tx, release_b_tx)
    };
    assert_eq!(
        queue.queue_shape(loser),
        (1, 1, 0),
        "the loser's read stays queued while the winner holds the permit"
    );
    release_winner.send(()).expect("release the winner");
    assert_eq!(
        done_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("the winner answers"),
        winner
    );
    assert_eq!(
        entered_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("the loser runs once the permit frees"),
        if winner == "a" { "b" } else { "a" }
    );
    release_loser.send(()).expect("release the loser");
    assert_eq!(
        done_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("the loser answers"),
        if winner == "a" { "b" } else { "a" }
    );
    assert_eq!(queue.queue_shape(loser), (0, 0, 0));
}

#[test]
fn reserved_read_permit_returned_when_job_not_run() {
    // A blocked head read takes nothing when a write overtakes it.
    let shared = Shared::default();
    set_read_permits(&shared, 0);
    let mut inner = Inner::default();
    inner.jobs.push_back(unit_read_job());
    inner.jobs.push_back(Job::Write(Box::new(|| {})));
    let Pick::Run(job) = next_runnable(&shared, &mut inner) else {
        panic!("the write overtakes the permit-less read");
    };
    assert!(
        matches!(job, Job::Write(_)),
        "the pick runs the write, not the blocked read"
    );
    assert_eq!(
        read_permits(&shared),
        0,
        "the overtake reserves no read permit"
    );
    assert_eq!(inner.jobs.len(), 1, "the blocked read keeps its place");

    // And nothing when the queue only waits.
    let mut lone = Inner::default();
    lone.jobs.push_back(unit_read_job());
    assert!(
        matches!(next_runnable(&shared, &mut lone), Pick::Wait(Lane::Read)),
        "without a permit the lone read waits"
    );
    assert_eq!(read_permits(&shared), 0);
    assert_eq!(lone.jobs.len(), 1);

    // The drain's guard hands a held permit back however the job ends.
    {
        let _held = PermitGuard {
            shared: &shared,
            lane: Lane::Read,
        };
    }
    assert_eq!(
        read_permits(&shared),
        1,
        "the dropped guard returns the permit"
    );
}
