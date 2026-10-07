//! Every lane reserves its permit at pick: two roots racing for the last
//! permit never both believe they hold it, and a pick that runs something
//! else takes nothing.

use std::sync::mpsc;
use std::time::Duration;

use super::*;

const RACE_ROOT_A: &str = "permit-race-a";
const RACE_ROOT_B: &str = "permit-race-b";

fn lane_permits(shared: &Shared, lane: Lane) -> usize {
    let (permits, _) = shared.lane(lane);
    *permits.lock().unwrap_or_else(|error| error.into_inner())
}

fn set_lane_permits(shared: &Shared, lane: Lane, count: usize) {
    let (permits, _) = shared.lane(lane);
    *permits.lock().unwrap_or_else(|error| error.into_inner()) = count;
}

fn restore_lanes(shared: &Shared) {
    set_lane_permits(shared, Lane::Read, MAX_IN_FLIGHT_READ_JOBS);
    set_lane_permits(shared, Lane::Write, MAX_IN_FLIGHT_WRITE_JOBS);
    set_lane_permits(shared, Lane::Value, MAX_IN_FLIGHT_VALUE_JOBS);
    for lane in [Lane::Read, Lane::Write, Lane::Value] {
        shared.lane(lane).1.notify_all();
    }
}

/// Owns one race queue and releases every gate on the way out, however the
/// test ends, before reaping its drains: a failing assertion must not strand
/// a parked drain or a gated compute.
struct RaceHarness {
    queue: GitQueue,
    park_release: Option<mpsc::Sender<()>>,
    compute_releases: Vec<mpsc::Sender<()>>,
}

impl RaceHarness {
    fn new() -> Self {
        Self {
            queue: GitQueue::default(),
            park_release: None,
            compute_releases: Vec::new(),
        }
    }
}

impl Drop for RaceHarness {
    fn drop(&mut self) {
        if let Some(release) = self.park_release.take() {
            let _ = release.send(());
        }
        for release in std::mem::take(&mut self.compute_releases) {
            let _ = release.send(());
        }
        restore_lanes(&self.queue.shared);
        join_drains(&self.queue.shared);
    }
}

fn pass_gate(
    entered: mpsc::Sender<String>,
    release: mpsc::Receiver<()>,
    done: mpsc::Sender<String>,
    name: String,
) {
    entered
        .send(name.clone())
        .expect("the harness holds the entered receiver");
    release.recv().expect("the harness releases the gate");
    done.send(name)
        .expect("the harness holds the done receiver");
}

fn gated_read_job(
    entered: mpsc::Sender<String>,
    release: mpsc::Receiver<()>,
    done: mpsc::Sender<String>,
    name: &str,
) -> Job {
    let name = name.to_string();
    Job::Read {
        key: ReadKey::new(1, None),
        compute: Box::new(move || {
            pass_gate(entered, release, done, name);
            DaemonMessage::Error(WireError::new(ErrorCode::Io, "test read"))
        }),
        sinks: Arc::new(Mutex::new(Vec::new())),
    }
}

fn gated_write_job(
    entered: mpsc::Sender<String>,
    release: mpsc::Receiver<()>,
    done: mpsc::Sender<String>,
    name: &str,
) -> Job {
    let name = name.to_string();
    Job::Write(Box::new(move || {
        pass_gate(entered, release, done, name);
    }))
}

fn unit_read_job() -> Job {
    Job::Read {
        key: ReadKey::new(7, None),
        compute: Box::new(|| DaemonMessage::Error(WireError::new(ErrorCode::Io, "test read"))),
        sinks: Arc::new(Mutex::new(Vec::new())),
    }
}

/// Two roots race for their lane's last permit while the winner's drain is
/// parked between pick and run. On the fixed code the loser stays queued and
/// runs once the winner's permit frees; on the check-then-acquire code both
/// drains pop, so the loser is gone from its queue and this fails.
fn check_last_permit_race(
    lane: Lane,
    make_job: impl Fn(mpsc::Sender<String>, mpsc::Receiver<()>, mpsc::Sender<String>, &str) -> Job,
    queued_shape: (usize, usize, usize),
) {
    let mut harness = RaceHarness::new();
    set_lane_permits(&harness.queue.shared, lane, 1);
    let (outcome_rx, park_tx) = arm_pick_park(&harness.queue.shared);
    harness.park_release = Some(park_tx);
    let (entered_tx, entered_rx) = mpsc::channel::<String>();
    let (done_tx, done_rx) = mpsc::channel::<String>();
    let (release_a_tx, release_a_rx) = mpsc::channel::<()>();
    let (release_b_tx, release_b_rx) = mpsc::channel::<()>();
    harness.compute_releases.push(release_a_tx.clone());
    harness.compute_releases.push(release_b_tx.clone());
    harness
        .queue
        .enqueue_job(
            RACE_ROOT_A.to_string(),
            make_job(entered_tx.clone(), release_a_rx, done_tx.clone(), "a"),
        )
        .expect("the worker spawns");
    // Root A's drain picks first and parks: nothing else is queued yet.
    let first_pick = outcome_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("root A's drain picks and parks");
    assert!(
        matches!(first_pick, PickOutcome::Ran),
        "root A's drain picks and parks, got {first_pick:?}"
    );
    harness
        .queue
        .enqueue_job(
            RACE_ROOT_B.to_string(),
            make_job(entered_tx, release_b_rx, done_tx, "b"),
        )
        .expect("the worker spawns");
    // Root B picks while A is parked between pick and run: waiting means it
    // found no permit, running means it popped without one.
    let second_pick = outcome_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("root B's drain picks while A is parked");
    assert!(
        matches!(second_pick, PickOutcome::Waited),
        "root B waits while A is parked holding the last permit, got {second_pick:?}"
    );
    disarm_pick_park(&harness.queue.shared);
    harness
        .park_release
        .take()
        .expect("the park is armed")
        .send(())
        .expect("release the parked drain");
    let first = entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the parked winner runs once released");
    assert_eq!(first, "a", "only the parked winner runs first");
    assert_eq!(
        harness.queue.queue_shape(RACE_ROOT_B),
        queued_shape,
        "the loser stays queued while the winner holds the permit"
    );
    if first == "a" {
        release_a_tx.send(()).expect("release the winner");
    } else {
        release_b_tx.send(()).expect("release the winner");
    }
    assert_eq!(
        done_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("the winner answers"),
        first
    );
    let second = entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the loser runs once the permit frees");
    assert_ne!(second, first, "the loser is the other root");
    if second == "a" {
        release_a_tx.send(()).expect("release the loser");
    } else {
        release_b_tx.send(()).expect("release the loser");
    }
    assert_eq!(
        done_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("the loser answers"),
        second
    );
    assert_eq!(harness.queue.queue_shape(RACE_ROOT_A), (0, 0, 0));
    assert_eq!(harness.queue.queue_shape(RACE_ROOT_B), (0, 0, 0));
}

#[test]
fn read_pick_reserves_its_permit() {
    check_last_permit_race(Lane::Read, gated_read_job, (1, 1, 0));
}

#[test]
fn write_pick_reserves_its_permit() {
    check_last_permit_race(Lane::Write, gated_write_job, (1, 0, 0));
}

#[test]
fn reserved_read_permit_returned_when_job_not_run() {
    // No reachable path reserves a permit for a job that is then not run:
    // the reservation and the pop are one step in `next_runnable`, and the
    // drain always runs a popped job. This pins both sides of that: a pick
    // that runs something else takes nothing, and a job that dies in
    // `compute` still hands its permit back.
    let shared = Shared::default();
    set_lane_permits(&shared, Lane::Read, 0);
    set_lane_permits(&shared, Lane::Write, 1);
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
    assert_eq!(lane_permits(&shared, Lane::Read), 0);
    assert_eq!(
        lane_permits(&shared, Lane::Write),
        0,
        "the overtake takes the write lane's permit for the write that runs"
    );
    assert_eq!(inner.jobs.len(), 1, "the blocked read keeps its place");

    let mut lone = Inner::default();
    lone.jobs.push_back(unit_read_job());
    assert!(
        matches!(next_runnable(&shared, &mut lone), Pick::Wait(Lane::Read)),
        "without a permit the lone read waits"
    );
    assert_eq!(lane_permits(&shared, Lane::Read), 0);
    assert_eq!(lone.jobs.len(), 1);

    // A read that panics in `compute` answers with an error and returns its
    // permit: the drain's guard drops however the job ends.
    let harness = RaceHarness::new();
    let (reply_tx, reply_rx) = mpsc::channel::<DaemonMessage>();
    harness
        .queue
        .enqueue_job(
            "panicking-read".to_string(),
            Job::Read {
                key: ReadKey::new(3, None),
                compute: Box::new(|| panic!("a test read panics")),
                sinks: Arc::new(Mutex::new(vec![Sink::new(Box::new(move |reply| {
                    reply_tx
                        .send(reply.clone())
                        .expect("the harness holds the reply receiver");
                }))])),
            },
        )
        .expect("the worker spawns");
    let reply = reply_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the panic becomes an error reply");
    assert!(
        matches!(reply, DaemonMessage::Error(_)),
        "a panicking compute answers with an error, got {reply:?}"
    );
    // The reply is delivered before the job's arm ends, so the guard may
    // not have dropped yet: joining first makes the count deterministic —
    // the drain exits only past the guard's drop. The harness reaps nothing
    // twice; its drop finds no handles left.
    join_drains(&harness.queue.shared);
    assert_eq!(
        lane_permits(&harness.queue.shared, Lane::Read),
        MAX_IN_FLIGHT_READ_JOBS,
        "the panicking job returns its permit"
    );
    assert_eq!(harness.queue.queue_shape("panicking-read"), (0, 0, 0));
}
