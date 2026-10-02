//! The queue tests' shared fixture: a registry, a live agent session whose
//! writer records what the daemon sent it, and the connections attached to it.
//!
//! One place because every queue test needs the same four things, and a
//! difference between two tests' fixtures is a difference nobody reads.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::*;

/// A registry on a temp dir with its journal, and the dir to clean up.
pub(super) fn queue_registry() -> (PathBuf, SessionRegistry, Arc<Journal>) {
    super::tests::tmp_delete_registry()
}

/// One fixture session and the four handles a queue test drives it through.
pub(super) struct QueuedSession {
    pub(super) owner: OwnerId,
    pub(super) id: String,
    pub(super) runtime: Arc<SessionRuntime>,
    /// Every byte this session's writer was handed.
    pub(super) sent: Arc<Mutex<Vec<u8>>>,
    pub(super) conn: Arc<ConnHandle>,
}

/// A live agent session with one attached client, idle: nothing has begun a
/// turn, so an add drains at once.
pub(super) fn queued_session(
    registry: &SessionRegistry,
    journal: &Arc<Journal>,
    user: &str,
    client: &str,
    tag: &str,
) -> QueuedSession {
    let owner = OwnerId::new(user, client).expect("owner");
    let id = compose_session_id(&owner.session_token(), tag).expect("id");
    let (runtime, sent) = queued_agent(registry, journal, &owner, &id);
    let conn = attached(registry, &id, 4, &owner);
    QueuedSession {
        owner,
        id,
        runtime,
        sent,
        conn,
    }
}

/// One live agent session whose sends land in `sent`, and the owner it belongs
/// to. The session starts idle: nothing has begun a turn on it.
///
/// The journal row is written here too, because a send journals its own prompt
/// and a session the journal has never heard of refuses that write — a queue
/// test that wanted to see what a send leaves behind would read nothing.
pub(super) fn queued_agent(
    registry: &SessionRegistry,
    journal: &Arc<Journal>,
    owner: &OwnerId,
    session_id: &str,
) -> (Arc<SessionRuntime>, Arc<Mutex<Vec<u8>>>) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let runtime = queued_agent_with_writer(
        registry,
        journal,
        owner,
        session_id,
        Box::new(QueueRecordingWriter(Arc::clone(&sent))),
    );
    (runtime, sent)
}

/// The same session with a writer the test supplies, so a send can be made to
/// block where it writes.
pub(super) fn queued_agent_with_writer(
    registry: &SessionRegistry,
    journal: &Arc<Journal>,
    owner: &OwnerId,
    session_id: &str,
    writer: Box<dyn Write + Send>,
) -> Arc<SessionRuntime> {
    queued_agent_counting_kills(registry, journal, owner, session_id, writer).0
}

/// The same session, and the count of times its killer was asked to kill, so a
/// test can tell a stop that reached the killer from one that is still waiting
/// to.
pub(super) fn queued_agent_counting_kills(
    registry: &SessionRegistry,
    journal: &Arc<Journal>,
    owner: &OwnerId,
    session_id: &str,
    writer: Box<dyn Write + Send>,
) -> (Arc<SessionRuntime>, Arc<AtomicUsize>) {
    journal_row(journal, session_id, owner);
    let kills = Arc::new(AtomicUsize::new(0));
    let killer = QueueKiller {
        runtime: Arc::new(OnceLock::new()),
        kills: Arc::clone(&kills),
    };
    let killer_slot = Arc::clone(&killer.runtime);
    let runtime = super::tests::insert_live_agent_with_turn_control(
        registry,
        session_id,
        owner.clone(),
        SessionKind::Claude,
        writer,
        None,
        None,
        Box::new(killer),
        Box::new(UnsupportedSteerer),
    );
    let _ = killer_slot.set(Arc::clone(&runtime));
    (runtime, kills)
}

/// The journal row a send journals its own prompt under. A session the journal
/// has never heard of refuses that write, which would leave a test with nothing
/// to read where the question is what a send leaves behind.
pub(super) fn journal_row(journal: &Arc<Journal>, session_id: &str, owner: &OwnerId) {
    journal
        .upsert_blocking(crate::journal::new_session_record(
            session_id,
            owner.user.as_str(),
            None,
            SessionKind::Claude,
            "Agent",
        ))
        .expect("the journal takes the session row");
}

/// The bytes one session's writer was handed, as text.
pub(super) fn sent_text(sent: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8(sent.lock().expect("sent lock").clone()).expect("utf8")
}

/// A connection attached to the session through the real attach road, so the
/// attach snapshot is published exactly as a client's is.
pub(super) fn attached(
    registry: &SessionRegistry,
    session_id: &str,
    conn_id: u64,
    owner: &OwnerId,
) -> Arc<ConnHandle> {
    let conn = ConnHandle::new(conn_id);
    registry
        .attach_with_subscription(session_id, conn_id, None, &conn, owner, false)
        .expect("attach");
    conn
}

/// One queue snapshot as the tests read it: what the client would have seen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SeenSnapshot {
    pub(super) revision: u64,
    pub(super) items: Vec<devboule_protocol::QueuedMessage>,
    /// The rows this one snapshot dropped, which is empty on every ordinary
    /// mutation.
    pub(super) dropped: Vec<String>,
}

/// The queue snapshots this connection has been given, oldest first.
pub(super) fn snapshots(conn: &ConnHandle) -> Vec<SeenSnapshot> {
    conn.pull_events()
        .into_iter()
        .filter_map(|pending| match pending.envelope.event {
            SessionEvent::QueueSnapshot {
                revision,
                items,
                dropped,
                ..
            } => Some(SeenSnapshot {
                revision,
                items,
                dropped: dropped.into_iter().map(|dropped| dropped.item_id).collect(),
            }),
            _ => None,
        })
        .collect()
}

/// The item ids this connection's latest snapshot carries.
pub(super) fn latest_ids(conn: &ConnHandle) -> Vec<String> {
    snapshots(conn)
        .pop()
        .map(|snapshot| {
            snapshot
                .items
                .into_iter()
                .map(|item| item.item_id)
                .collect()
        })
        .unwrap_or_default()
}

/// A connection that attached without negotiating `session.queue`, which is
/// what a client build from before protocol 22 does.
pub(super) fn attached_without_queue_capability(
    registry: &SessionRegistry,
    session_id: &str,
    conn_id: u64,
    owner: &OwnerId,
) -> Arc<ConnHandle> {
    let conn = ConnHandle::new(conn_id);
    conn.set_session_queue_negotiated(false);
    registry
        .attach_with_subscription(session_id, conn_id, None, &conn, owner, false)
        .expect("attach");
    conn
}

/// Wait for `ready`, polling: the drain that answers a turn end runs on its own
/// thread, so a test that asserts on what it wrote cannot know when.
pub(super) fn eventually(label: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {label}");
}

/// End the session's running turn the way a provider does: the finish event the
/// daemon turns into the turn's end, which is also what fires the queue's arm.
pub(super) fn end_turn(runtime: &Arc<SessionRuntime>) {
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
}

/// A writer that parks inside its first write until the test releases it, so a
/// close can land while a send is genuinely on the wire. `first_write_seen`
/// turns true the moment the write reached it, which is how a test knows the
/// send is blocked rather than merely slow.
pub(super) struct BlockingWriter {
    pub(super) bytes: Arc<Mutex<Vec<u8>>>,
    pub(super) first_write_seen: Arc<AtomicBool>,
    pub(super) release: Mutex<mpsc::Receiver<()>>,
}

/// The handle a test keeps to let a [`BlockingWriter`] finish its first write.
pub(super) fn blocking_writer() -> (BlockingWriter, Sender<()>) {
    let (release, receiver) = mpsc::channel();
    (
        BlockingWriter {
            bytes: Arc::new(Mutex::new(Vec::new())),
            first_write_seen: Arc::new(AtomicBool::new(false)),
            release: Mutex::new(receiver),
        },
        release,
    )
}

impl Write for BlockingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes
            .lock()
            .expect("blocking writer lock")
            .extend_from_slice(bytes);
        if !self.first_write_seen.swap(true, Ordering::AcqRel) {
            let release = self.release.lock().expect("release lock");
            // Bounded, and a dropped sender also releases it: a test that
            // fails must not leave a send parked in this writer, and a second
            // send that arrives while the first was released must not park
            // here either.
            let _ = release.recv_timeout(Duration::from_secs(5));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A writer that refuses every write, the way a provider's stdin refuses once
/// the child is gone. The failure comes from the write itself, which is the
/// case a queue must never retry on its own.
pub(super) struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("forced writer failure"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A writer that is the ACP road's shape at flush time: the text is buffered
/// and the flush finds the child's stdin already closed, which is what the ACP
/// writer hands the send path once the sibling process is gone. The failure
/// comes from the real [`write_child_stdin`] with a `None` stdin, so it is the
/// marker the production path produces and not a stand-in for it.
#[derive(Default)]
pub(super) struct ClosedStdinWriter {
    pending: Vec<u8>,
}

impl Write for ClosedStdinWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let stdin = Mutex::new(None);
        super::write_child_stdin(&stdin, &std::mem::take(&mut self.pending), "ACP")
    }
}

struct QueueRecordingWriter(Arc<Mutex<Vec<u8>>>);

impl Write for QueueRecordingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("recording writer lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The provider's half of an interrupt: the abort reaches the daemon as the
/// finish that ends the turn, which is what a send-now's wait is waiting for.
/// The runtime is wired in afterwards, because the killer is built before the
/// runtime it acks.
///
/// The finish is published from a thread of its own, as a real provider's
/// reader thread reports it. It cannot be published inline: the interrupt send
/// runs the killer under the runtime's turn-hold, and a finish taken on that
/// same thread would want the same lock.
pub(super) struct QueueKiller {
    runtime: Arc<OnceLock<Arc<SessionRuntime>>>,
    kills: Arc<AtomicUsize>,
}

impl SessionKiller for QueueKiller {
    fn kill(&mut self) {
        self.kills.fetch_add(1, Ordering::SeqCst);
    }

    fn interrupt(&mut self) {
        let Some(runtime) = self.runtime.get().cloned() else {
            return;
        };
        let _ = std::thread::Builder::new()
            .name("queue-fixture-interrupt-ack".to_string())
            .spawn(move || {
                runtime.publish_agent_event(
                    SessionEvent::AgentFinished {
                        stop_reason: "interrupt".to_string(),
                        model_id: None,
                        usage: None,
                    },
                    None,
                );
            });
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self {
            runtime: Arc::clone(&self.runtime),
            kills: Arc::clone(&self.kills),
        })
    }
}
