//! The substrate under the workspace git arms: one FIFO per repository
//! root, two permit lanes, per-root caps on queued and waitlisted reads,
//! and the write-drain that shutdown waits on. This file knows nothing
//! about `ClientMessage` — a job carries an opaque read key, a closure that
//! produces the reply, and the sinks that receive it; read-versus-write is
//! the job's own variant, which is what the lanes and the drain key on.
//! The request side (`git_workers.rs`) classifies arms, builds keys and
//! sinks, and owns every wire shape.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use devboule_protocol::DaemonMessage;

use super::*;

/// How long shutdown waits for accepted write jobs before it flushes the
/// journal anyway. Long enough for a `git worktree remove` on a healthy
/// repository; bounded so a pathological removal cannot hang the exit.
pub(super) const GIT_WRITE_DRAIN_BOUND: Duration = Duration::from_secs(10);

/// The read lane: status/diff/log polls. The sidebar polls one status per
/// visible workspace on a timer, so without a process-wide ceiling that
/// sweep would run one git process per workspace at once — the thing a slow
/// network folder tolerates worst. Four keeps a sweep four processes deep
/// while independent workspaces still make progress.
pub(super) const MAX_IN_FLIGHT_READ_JOBS: usize = 4;
/// The write lane: stage, commit, discard, rename, delete, create, project
/// add. Writes are user-initiated and must not queue behind the read lane
/// (a commit stuck behind four slow statuses is a timeout that later lands),
/// and they get their own small ceiling against bursts; same-root index
/// acts stay serial through `git_write_lock` regardless of this number.
/// The price is declared: a read whose lane has no permit is passed over by
/// every write its own root produces for as long as they keep arriving —
/// a poll is re-issued on its timer, a person's commit is not re-issued at
/// all, so the write wins.
pub(super) const MAX_IN_FLIGHT_WRITE_JOBS: usize = 2;
/// The value lane: a read that answers a value instead of a wire frame (a
/// repository sweep) rather than a poll. Its own small ceiling, because one
/// such job runs several git commands under a single permit for as long as the
/// sweep lasts — four of them would hold the whole read lane, the sidebar's
/// status and diff polls included. Two keeps two sweeps answering without
/// spending a permit the workspace reads need. A sweep with no permit waits
/// its turn in the queue like any other blocked job: it is never picked up
/// only to block the drain, so a write behind it still runs.
pub(super) const MAX_IN_FLIGHT_VALUE_JOBS: usize = 2;
/// Queued read jobs allowed per root. Beyond it a new poll waits on the
/// root's waitlist and is admitted when a read on that root frees its
/// queued slot — it is then a fresh read that runs after everything already
/// queued, so overflow never answers with a tree a queued write has not
/// happened in. Past the waitlist's own cap the newest read supersedes the
/// oldest (see [`WAITING_READ_CAP`]); nothing is refused.
pub(super) const QUEUED_READ_CAP: usize = 4;
/// Reads allowed on one root's waitlist. The timer's polls coalesce, so the
/// waitlist only grows under distinct-path floods — a user clicking through
/// file diffs — and past this bound the oldest waiting read is superseded
/// by the newest, whose answer is the one the user is waiting for.
pub(super) const WAITING_READ_CAP: usize = 12;

/// How long a drain with nothing runnable sleeps before it re-picks its
/// queue. A freed read permit wakes it at once; this poll bounds only the
/// wait for a write arriving behind a read that cannot take its permit.
const PICK_POLL: Duration = Duration::from_millis(50);
/// The ceiling on any [`GitQueue::read_value`] wait, whatever the caller asks
/// for: a wedged queue must refuse a call rather than hold its thread for
/// ever. A caller that passes its own bound below this gets that bound.
const VALUE_WAIT: Duration = Duration::from_secs(30);

/// One queued unit of git work.
pub(super) enum Job {
    /// A read-only poll. A later poll may join it while it is the queue's
    /// last job (never across a write), or join an identical read on the
    /// waitlist, and every joined request is answered from this job's one
    /// result.
    Read {
        key: ReadKey,
        compute: Box<dyn FnOnce() -> DaemonMessage + Send>,
        sinks: Arc<Mutex<Vec<Sink>>>,
    },
    /// A mutation or any non-poll arm: the closure answers its own caller.
    Write(Box<dyn FnOnce() + Send>),
    /// A read that answers a value rather than a wire frame: an MCP tool
    /// runs git and has no [`DaemonMessage`] to deliver. Runs in this root's
    /// queue order on its own lane ([`MAX_IN_FLIGHT_VALUE_JOBS`]), coalescing
    /// nothing, and like a read it waits for a permit in the queue rather
    /// than in the drain.
    ///
    /// Its compute MUST NOT enqueue another job on this root and wait for
    /// it: one root has one drain thread, so such a job would sit behind this
    /// one forever.
    Value(Box<dyn FnOnce() + Send>),
}

/// What one queued read is a repeat of, opaque to the substrate: the
/// request side derives the tag from the frame kind and adds the diff's
/// path where the frame has one.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct ReadKey {
    tag: u64,
    path: Option<String>,
}

impl ReadKey {
    pub(super) fn new(tag: u64, path: Option<String>) -> Self {
        Self { tag, path }
    }
}

/// One request waiting on a read job's result. The delivery closure is
/// request side; the substrate only calls it with the job's result.
pub(super) struct Sink {
    deliver: Box<dyn Fn(&DaemonMessage) + Send>,
}

impl Sink {
    pub(super) fn new(deliver: Box<dyn Fn(&DaemonMessage) + Send>) -> Self {
        Self { deliver }
    }

    fn deliver(&self, reply: &DaemonMessage) {
        (self.deliver)(reply);
    }
}

/// The per-root queues, the lanes' permits, and the in-flight write count,
/// on the state so every connection shares them.
#[derive(Default)]
pub(super) struct GitQueue {
    shared: Arc<Shared>,
}

struct Shared {
    roots: Mutex<HashMap<String, Arc<RootQueue>>>,
    read_permits: Mutex<usize>,
    read_freed: Condvar,
    write_permits: Mutex<usize>,
    write_freed: Condvar,
    value_permits: Mutex<usize>,
    value_freed: Condvar,
    /// Write jobs accepted and not finished — queued behind slower work or
    /// running. Shutdown waits for zero, bounded, before it flushes the
    /// journal, so a delete that has not started yet still lands its row.
    writes_outstanding: Mutex<usize>,
    write_done: Condvar,
    /// Set when the shutdown drain gave up on its bound: writes still queued
    /// at that moment are answered instead of run, because the flush they
    /// were meant to precede is next.
    writes_cancelled: AtomicBool,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            roots: Mutex::new(HashMap::new()),
            read_permits: Mutex::new(MAX_IN_FLIGHT_READ_JOBS),
            read_freed: Condvar::new(),
            write_permits: Mutex::new(MAX_IN_FLIGHT_WRITE_JOBS),
            write_freed: Condvar::new(),
            value_permits: Mutex::new(MAX_IN_FLIGHT_VALUE_JOBS),
            value_freed: Condvar::new(),
            writes_outstanding: Mutex::new(0),
            write_done: Condvar::new(),
            writes_cancelled: AtomicBool::new(false),
        }
    }
}

#[derive(Default)]
struct RootQueue {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    jobs: VecDeque<Job>,
    /// Reads that arrived at the [`QUEUED_READ_CAP`] or behind other
    /// waitlisted reads. The map is the O(1) join and lookup; the deque is
    /// the arrival order the map cannot give — newest admitted first, and
    /// the oldest evicted when the [`WAITING_READ_CAP`] is hit. The two are
    /// only ever mutated together, under `inner`, and every mutation site
    /// asserts the pairing.
    waiting: HashMap<ReadKey, Job>,
    waiting_order: VecDeque<ReadKey>,
    /// A worker is draining this queue. Guarded by `inner` together with
    /// `jobs`, so "busy" is never stale: the worker clears it only under the
    /// lock where it also observed the queue empty, and anything pushed
    /// after that moment saw `busy == false` and armed its own worker.
    busy: bool,
}

/// Clears the queue's busy flag however the drain ends. The normal exit
/// disarms it after clearing the flag itself; an unwind drops it armed, so
/// a dying worker can never leave a root's queue armed and dead.
struct DrainGuard {
    queue: Arc<RootQueue>,
    armed: bool,
}

impl Drop for DrainGuard {
    fn drop(&mut self) {
        if self.armed {
            let mut inner = self
                .queue
                .inner
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            inner.busy = false;
        }
    }
}

#[derive(Clone, Copy)]
enum Lane {
    Read,
    Write,
    Value,
}

/// Returns the lane's permit on the way out, however the job ends.
struct PermitGuard<'a> {
    shared: &'a Shared,
    lane: Lane,
}

impl Drop for PermitGuard<'_> {
    fn drop(&mut self) {
        let (permits, freed) = self.shared.lane(self.lane);
        let mut permits = permits.lock().unwrap_or_else(|error| error.into_inner());
        *permits += 1;
        freed.notify_all();
    }
}

/// Counts one accepted write down on the way out, however the job ends.
/// Created when the drain starts the job, against the count taken at
/// enqueue; a job that never reaches its drain (a failed spawn) is counted
/// down on that road instead.
struct WriteFinishedGuard<'a> {
    shared: &'a Shared,
}

impl Drop for WriteFinishedGuard<'_> {
    fn drop(&mut self) {
        self.shared.write_finished();
    }
}

impl Shared {
    fn lane(&self, lane: Lane) -> (&Mutex<usize>, &Condvar) {
        match lane {
            Lane::Read => (&self.read_permits, &self.read_freed),
            Lane::Write => (&self.write_permits, &self.write_freed),
            Lane::Value => (&self.value_permits, &self.value_freed),
        }
    }

    /// One more write accepted: counted from enqueue, so a job still
    /// queued behind a slow read is drained by shutdown too.
    fn write_accepted(&self) {
        let mut outstanding = self
            .writes_outstanding
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *outstanding += 1;
    }

    fn write_finished(&self) {
        let mut outstanding = self
            .writes_outstanding
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *outstanding -= 1;
        self.write_done.notify_all();
    }
}

fn acquire(shared: &Shared, lane: Lane) {
    let (permits, freed) = shared.lane(lane);
    let mut permits = permits.lock().unwrap_or_else(|error| error.into_inner());
    while *permits == 0 {
        permits = freed
            .wait(permits)
            .unwrap_or_else(|error| error.into_inner());
    }
    *permits -= 1;
}

fn queued_reads(inner: &Inner) -> usize {
    inner
        .jobs
        .iter()
        .filter(|job| matches!(job, Job::Read { .. }))
        .count()
}

/// What the drain does with this root's queue next.
enum Pick {
    Run(Job),
    /// The head is a job whose lane has no permit: sleep until that lane
    /// frees one or the pick poll elapses, then look again.
    Wait(Lane),
    Exit,
}

/// Take `lane`'s permit if it has one to spare, so that taking it and acting
/// on it are one step: two drains can never both believe they hold the last
/// one, which is what would let a job block its root's queue on a permit it
/// believed it had.
fn reserve_permit(shared: &Shared, lane: Lane) -> bool {
    let (permits, _) = shared.lane(lane);
    let mut permits = permits.lock().unwrap_or_else(|error| error.into_inner());
    if *permits == 0 {
        return false;
    }
    *permits -= 1;
    true
}

/// The queue's next job, popped. Waitlisted reads first fill whatever
/// queued slots [`QUEUED_READ_CAP`] has free — behind everything queued
/// now, newest admitted first, because the read the user just asked for is
/// the one whose latency they feel. The head runs, unless its lane has no
/// permit and the queue holds a write: the write then runs ahead of the
/// blocked job, unconditionally, and the blocked job keeps its place. The
/// reordering is only ever of a write ahead — every other job then observes
/// the write, which is fresher than the tree it was queued against, never
/// staler, and a value job never overtakes a read either.
fn next_runnable(shared: &Shared, inner: &mut Inner) -> Pick {
    while queued_reads(inner) < QUEUED_READ_CAP {
        let Some(key) = inner.waiting_order.pop_back() else {
            break;
        };
        match inner.waiting.remove(&key) {
            Some(job) => inner.jobs.push_back(job),
            None => break,
        }
        debug_assert_eq!(inner.waiting_order.len(), inner.waiting.len());
    }
    let Some(head) = inner.jobs.front() else {
        return Pick::Exit;
    };
    // A write takes the write lane's permit in the drain, which is where that
    // wait has always lived. A read's and a value job's permits are taken
    // here, with the pick: the drain then runs them without taking one, so a
    // read or sweep with no permit to spare waits in this queue rather than
    // blocking the drain that also has to serve this root's writes.
    let blocked = match head {
        Job::Write(_) => false,
        Job::Read { .. } => !reserve_permit(shared, Lane::Read),
        Job::Value(_) => !reserve_permit(shared, Lane::Value),
    };
    if !blocked {
        return Pick::Run(inner.jobs.pop_front().expect("front checked"));
    }
    if let Some(index) = inner
        .jobs
        .iter()
        .position(|job| matches!(job, Job::Write(_)))
    {
        return Pick::Run(inner.jobs.remove(index).expect("position checked"));
    }
    Pick::Wait(match head {
        Job::Read { .. } => Lane::Read,
        Job::Value(_) => Lane::Value,
        Job::Write(_) => Lane::Write,
    })
}

/// The reply's kind for the sink-panic log line: the read shapes the queue
/// delivers, plus the plain error frame.
fn reply_kind(reply: &DaemonMessage) -> &'static str {
    match reply {
        DaemonMessage::WorkspaceGit { .. } => "WorkspaceGit",
        DaemonMessage::WorkspaceGitFile { .. } => "WorkspaceGitFile",
        DaemonMessage::WorkspaceGitLog { .. } => "WorkspaceGitLog",
        DaemonMessage::Error(_) => "Error",
        _ => "other",
    }
}

/// Deliver `reply` to every sink, each inside its own catch: a panicking
/// sink must not drop the joiners queued behind it. A caught panic is
/// logged once, naming the reply's kind and nothing of its payload — the
/// request whose answer it was is one a client is waiting on.
fn deliver_all(sinks: Arc<Mutex<Vec<Sink>>>, reply: &DaemonMessage) {
    for sink in sinks
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .drain(..)
    {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink.deliver(reply))).is_err() {
            eprintln!(
                "git queue: a sink panicked delivering a {} reply; that request's answer was dropped",
                reply_kind(reply)
            );
        }
    }
}

/// Join an identical read to this root's last queued job — only when it is
/// a read, so a join never crosses a write — or to the identical read on
/// the waitlist, which is one job per key. Both positions answer after
/// every write queued before this call: a waitlisted read is admitted to
/// the back of the queue, behind whatever is queued now. `false` means
/// nothing identical is waiting.
fn join_read(inner: &Inner, key: &ReadKey, sinks: &Arc<Mutex<Vec<Sink>>>) -> bool {
    let target = match inner.jobs.back() {
        Some(Job::Read {
            key: queued,
            sinks: queued_sinks,
            ..
        }) if queued == key => queued_sinks,
        _ => match inner.waiting.get(key) {
            Some(Job::Read {
                sinks: queued_sinks,
                ..
            }) => queued_sinks,
            Some(Job::Write(_) | Job::Value(_)) | None => return false,
        },
    };
    let mut incoming = sinks.lock().unwrap_or_else(|error| error.into_inner());
    target
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .append(&mut incoming);
    true
}

impl GitQueue {
    /// Run `compute` on `root`'s queue and hand its value back to the caller,
    /// waiting at most `within` (and never past this queue's own ceiling).
    /// Nothing coalesces on this key and nothing is counted as a write; the
    /// per-root order is the same one the workspace git arms take, so a sweep
    /// observes the tree as the writes before it left it, and the lane is its
    /// own so a long sweep never holds a permit a status poll needs.
    ///
    /// `within` is the caller's own bound on its own work, which is what keeps
    /// the two honest: a job that overruns it is reported as a refusal, and
    /// that job still runs to its end on the drain thread.
    ///
    /// The caller blocks here, on the thread that asked the question. `Err`
    /// means the worker could not start the job, or the bound ran out with it
    /// still queued — never a partial value.
    pub(super) fn read_value<T, F>(
        &self,
        root: String,
        within: Duration,
        compute: F,
    ) -> Result<T, &'static str>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let (answer_tx, answer_rx) = std::sync::mpsc::sync_channel(1);
        let job = Job::Value(Box::new(move || {
            // A panic inside the compute drops the sender, and the caller
            // reads that as the refusal it is.
            let _ = answer_tx.send(compute());
        }));
        self.enqueue_job(root, job)?;
        answer_rx
            .recv_timeout(within.min(VALUE_WAIT))
            .map_err(|_| "the repository's git queue did not answer in time")
    }

    /// Queue one git job for `root`, spawning this root's worker when
    /// idle. A read joins the queue's last job only when that job is the
    /// same read — a join past a write would answer from before the write —
    /// or the identical read on the waitlist. A read at the per-root cap,
    /// or behind waitlisted reads, waits on the waitlist; a read past the
    /// waitlist's own cap supersedes the oldest waiting read, whose sinks
    /// are answered with a superseded error (the frontend discards stale
    /// reads by generation) and which is never refused. `Err` means the
    /// worker could not be started and `job` was not accepted — the caller
    /// owes the request an error reply.
    pub(super) fn enqueue_job(&self, root: String, job: Job) -> Result<(), &'static str> {
        const SUPERSEDED_READS: &str =
            "too many pending git reads for this workspace; superseded by newer reads";
        let queue = {
            let mut roots = self
                .shared
                .roots
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            Arc::clone(roots.entry(root).or_default())
        };
        let writes_a_job = matches!(job, Job::Write(_));
        if writes_a_job {
            self.shared.write_accepted();
        }
        let mut job = Some(job);
        let mut evicted = None;
        let mut armed = false;
        {
            let mut inner = queue
                .inner
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            // Only a live drain may absorb a join: joining an idle root's
            // leftover queue would answer this request without arming the
            // worker that must drain the rest.
            if inner.busy {
                if let Some(Job::Read { key, sinks, .. }) = job.as_mut() {
                    if join_read(&inner, key, sinks) {
                        return Ok(());
                    }
                }
                let waitlist_key = match &job {
                    Some(Job::Read { key, .. })
                        if !inner.waiting.is_empty() || queued_reads(&inner) >= QUEUED_READ_CAP =>
                    {
                        Some(key.clone())
                    }
                    _ => None,
                };
                if let Some(key) = waitlist_key {
                    if inner.waiting.len() >= WAITING_READ_CAP {
                        // The oldest waiting read makes room: the user has
                        // clicked past it, and the newest read is the one
                        // whose answer is still wanted.
                        let oldest = inner
                            .waiting_order
                            .pop_front()
                            .expect("the order tracks the map");
                        evicted = inner.waiting.remove(&oldest);
                        debug_assert_eq!(inner.waiting_order.len(), inner.waiting.len());
                    }
                    inner.waiting_order.push_back(key.clone());
                    inner
                        .waiting
                        .insert(key, job.take().expect("a waitlisted job is held"));
                    debug_assert_eq!(inner.waiting_order.len(), inner.waiting.len());
                } else {
                    inner
                        .jobs
                        .push_back(job.take().expect("a queued job is held"));
                }
            } else {
                inner.busy = true;
                armed = true;
                // The queue is the drain's only source of work from here
                // on; the front is this job until the spawned drain pops
                // it.
                inner
                    .jobs
                    .push_front(job.take().expect("job held for the arm"));
            }
        }
        if let Some(Job::Read { sinks, .. }) = evicted {
            // The superseded read's people have clicked past it; the frame
            // is the same error kind the refusal used, restamped per sink
            // with its own id, and the client's generation check buries it.
            deliver_all(
                sinks,
                &DaemonMessage::Error(WireError::new(ErrorCode::Io, SUPERSEDED_READS)),
            );
        }
        // Only the enqueue that armed the queue starts the worker: an
        // enqueue that found the queue busy — queued or waitlisted —
        // belongs to the drain already running over it.
        if !armed {
            return Ok(());
        }
        let spawn = std::thread::Builder::new()
            .name("daemon-git-workspace".to_string())
            .spawn({
                let shared = Arc::clone(&self.shared);
                let queue = Arc::clone(&queue);
                move || drain(shared, queue)
            });
        if spawn.is_err() {
            let mut inner = queue
                .inner
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            inner.busy = false;
            if let Some(unrun) = inner.jobs.pop_front() {
                // The job was never run; a write's count is taken at enqueue
                // and closes here, and a value job's sender is dropped with
                // the job, so its caller reads a refusal instead of waiting.
                if matches!(unrun, Job::Write(_)) {
                    self.shared.write_finished();
                }
            }
            return Err("could not start the workspace git request");
        }
        Ok(())
    }

    /// Wait until every accepted write job has finished — queued ones
    /// included, so a delete parked behind a slow read is drained too — for
    /// at most `bound`. Shutdown calls this before it flushes the journal,
    /// so a workspace delete's journal write is not pulled out from under
    /// by the writer being joined. `false` means the bound ran out with
    /// writes still going.
    pub(super) fn wait_for_write_jobs(&self, bound: Duration) -> bool {
        let start = Instant::now();
        let mut outstanding = self
            .shared
            .writes_outstanding
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while *outstanding > 0 {
            let Some(remaining) = bound.checked_sub(start.elapsed()) else {
                return false;
            };
            let (guard, timeout) = self
                .shared
                .write_done
                .wait_timeout(outstanding, remaining)
                .unwrap_or_else(|error| error.into_inner());
            outstanding = guard;
            if timeout.timed_out() && *outstanding > 0 {
                return false;
            }
        }
        true
    }

    /// The drain bound ran out with writes still queued: they must answer
    /// instead of run, because the flush they were meant to precede is
    /// next. Already-running writes are past this point and finish.
    pub(super) fn cancel_queued_writes(&self) {
        self.shared.writes_cancelled.store(true, Ordering::SeqCst);
    }

    pub(super) fn writes_cancelled(&self) -> bool {
        self.shared.writes_cancelled.load(Ordering::SeqCst)
    }

    /// `(queued jobs, queued reads, waitlisted reads)` for one root — the
    /// white-box view the bound and coalescing tests assert on.
    #[cfg(test)]
    pub(super) fn queue_shape(&self, root: &str) -> (usize, usize, usize) {
        let roots = self
            .shared
            .roots
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(queue) = roots.get(root) else {
            return (0, 0, 0);
        };
        let inner = queue
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        (inner.jobs.len(), queued_reads(&inner), inner.waiting.len())
    }
}

/// Run this root's queued jobs one at a time, in queue order — except that
/// a read or value job with no permit to take steps aside for the root's
/// own writes ([`next_runnable`]). A picked read or value job arrives
/// holding its lane's permit; a write waits for its lane's permit before it
/// starts, so the concurrently running git count is bounded per lane no
/// matter how many roots have work.
fn drain(shared: Arc<Shared>, queue: Arc<RootQueue>) {
    let mut guard = DrainGuard {
        queue: Arc::clone(&queue),
        armed: true,
    };
    loop {
        let job = loop {
            let mut inner = queue
                .inner
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match next_runnable(&shared, &mut inner) {
                Pick::Run(job) => break job,
                Pick::Exit => {
                    inner.busy = false;
                    guard.armed = false;
                    return;
                }
                Pick::Wait(lane) => {
                    drop(inner);
                    let (permits, freed) = shared.lane(lane);
                    let permits = permits.lock().unwrap_or_else(|error| error.into_inner());
                    if *permits > 0 {
                        continue;
                    }
                    let (woken, _) = freed
                        .wait_timeout(permits, PICK_POLL)
                        .unwrap_or_else(|error| error.into_inner());
                    drop(woken);
                }
            }
        };
        // A panicking job or sink is caught here: the drain itself must
        // survive it, or one bad job would strand every later one.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
            Job::Read {
                key: _,
                compute,
                sinks,
            } => {
                // The permit came with the pick, so this guard only hands it
                // back once the read is done with it.
                let _permit = PermitGuard {
                    shared: &shared,
                    lane: Lane::Read,
                };
                let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(compute))
                    .unwrap_or_else(|_| {
                        DaemonMessage::Error(WireError::new(
                            ErrorCode::Io,
                            "the workspace git request failed",
                        ))
                    });
                deliver_all(sinks, &reply);
            }
            Job::Write(run) => {
                // The count was taken at enqueue; this guard closes it
                // however the job ends, after any permit wait.
                let _finished = WriteFinishedGuard { shared: &shared };
                acquire(&shared, Lane::Write);
                {
                    let _permit = PermitGuard {
                        shared: &shared,
                        lane: Lane::Write,
                    };
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
                }
            }
            Job::Value(run) => {
                // The permit came with the pick, so this guard only hands it
                // back once the sweep is done with it.
                let _permit = PermitGuard {
                    shared: &shared,
                    lane: Lane::Value,
                };
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
            }
        }));
    }
}

/// The read lane's pick-and-take, beside the substrate it pins.
#[cfg(test)]
#[path = "git_queue_read_permit_tests.rs"]
mod read_permit_tests;
