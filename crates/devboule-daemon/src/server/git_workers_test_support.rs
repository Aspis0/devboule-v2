//! Shared fixtures for the git-off-loop tests: a state with a registered
//! workspace, a real repository, the command-gate arm, the reply log that
//! records production order on both roads, and the worker-reply wait. The
//! tests are split by topic across the `git_workers_*_tests` files.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

use devboule_protocol::DaemonMessage;

use super::*;

pub(super) fn temp_state(tag: &str) -> (std::path::PathBuf, Arc<ServerState>) {
    let path = crate::test_dirs::test_temp_dir(&format!("devboule-{tag}"));
    let state = ServerState::with_paths(
        "test-instance".to_string(),
        RuntimePaths::from_dir(path.clone()),
    )
    .expect("state");
    (path, state)
}

/// Register one more Local workspace on `root`, on an existing state — the
/// queues live on the state, so tests with several workspaces register them
/// all on one.
pub(super) fn add_workspace(state: &ServerState, root: &std::path::Path) -> String {
    let project = state
        .sessions
        .project_add(root.to_str().expect("root path"))
        .expect("project");
    state
        .sessions
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("workspace")
        .id
}

/// A state with one Local workspace registered on `root`, whose id the git
/// arms resolve the way the app sends them.
pub(super) fn state_with_workspace(
    tag: &str,
    root: &std::path::Path,
) -> (PathBuf, Arc<ServerState>, String) {
    let (path, state) = temp_state(tag);
    let workspace_id = add_workspace(&state, root);
    (path, state, workspace_id)
}

pub(super) fn test_owner() -> OwnerId {
    OwnerId::new("alex", "app").expect("owner")
}

/// Every reply a test's requests produced, in production order: one
/// collector thread per connection records worker-road arrivals, and a
/// dispatch that ran inline records its own return at the moment it ran.
/// Ordering tests assert on `ids()`; content tests look replies up by id.
#[derive(Clone)]
pub(super) struct ReplyStore(Arc<Mutex<Vec<(u64, DaemonMessage)>>>);

impl ReplyStore {
    pub(super) fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    fn record(&self, id: u64, reply: DaemonMessage) {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((id, reply));
    }

    pub(super) fn ids(&self) -> Vec<u64> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .map(|(id, _)| *id)
            .collect()
    }

    pub(super) fn get(&self, id: u64) -> DaemonMessage {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .find(|(reply_id, _)| *reply_id == id)
            .expect("reply for id")
            .1
            .clone()
    }

    /// Poll until `n` replies have been recorded. The bound only catches a
    /// lost reply: every reply is real git work, and the gate runs these
    /// repos in parallel.
    pub(super) fn wait_len(&self, n: usize) {
        self.wait_len_within(n, Duration::from_secs(30));
    }

    /// Every one of `total` replies, one bounded wait per reply: the root
    /// drains serially, so from one reply to the next lies exactly one
    /// read's git work — a burst must not have to fit one window.
    pub(super) fn wait_each_reply(&self, total: usize) {
        for answered in 1..=total {
            self.wait_len(answered);
        }
    }

    /// The same wait under an explicit budget: a released burst drains its
    /// reads one at a time through real git processes, which on a loaded box
    /// legitimately takes longer than the default.
    pub(super) fn wait_len_within(&self, n: usize, budget: Duration) {
        let deadline = Instant::now() + budget;
        loop {
            if self.ids().len() >= n {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "only {} of {n} replies arrived: {:?}",
                self.ids().len(),
                self.ids()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn reply_id(reply: &DaemonMessage) -> Option<u64> {
    match reply {
        DaemonMessage::WorkspaceGit { id, .. }
        | DaemonMessage::WorkspaceGitLog { id, .. }
        | DaemonMessage::WorkspaceGitFile { id, .. }
        | DaemonMessage::WorkspaceGitWrite { id, .. } => Some(*id),
        DaemonMessage::Error(error) => error.id,
        _ => None,
    }
}

/// Wait until exactly `expected` of the gates have been taken, polling
/// instead of sleeping a fixed spell; the gates stay held while sampling, so
/// an over-cap start would show up in `held`.
pub(super) fn wait_until_taken(
    gates: &[(
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::SyncSender<()>,
    )],
    expected: usize,
) -> Vec<bool> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut held = vec![false; gates.len()];
    loop {
        for (index, (entered, _)) in gates.iter().enumerate() {
            if !held[index] && entered.try_recv().is_ok() {
                held[index] = true;
            }
        }
        if held.iter().filter(|taken| **taken).count() == expected {
            return held;
        }
        assert!(
            Instant::now() < deadline,
            "only {} of {expected} expected commands started: {held:?}",
            held.iter().filter(|taken| **taken).count()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Record every reply this connection produces, in arrival order, until the
/// returned flag is set. Stop it only after the expected replies are in the
/// store.
pub(super) fn spawn_collector(
    conn: &Arc<ConnHandle>,
    store: ReplyStore,
) -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let stop = Arc::new(AtomicBool::new(false));
    let conn = Arc::clone(conn);
    let flag = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let mut backlog = VecDeque::new();
        while !flag.load(std::sync::atomic::Ordering::SeqCst) {
            backlog.extend(conn.outbound.pull_replies());
            while let Some(reply) = backlog.pop_front() {
                if let Some(id) = reply_id(&reply) {
                    store.record(id, reply);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    (stop, handle)
}

/// Dispatch `request` on its own thread. A worker-road reply is recorded by
/// the connection's collector; an inline-road reply is recorded here, at the
/// moment it was produced.
pub(super) fn spawn_dispatch(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    request: ClientMessage,
    conn: &Arc<ConnHandle>,
    store: ReplyStore,
) -> std::thread::JoinHandle<()> {
    let state = Arc::clone(state);
    let owner = owner.clone();
    let conn = Arc::clone(conn);
    std::thread::spawn(move || {
        match dispatch(&state, &owner, request, &conn, true, true, true, true) {
            Some(reply) => {
                if let Some(id) = reply_id(&reply) {
                    store.record(id, reply);
                }
            }
            None => {
                // The worker road answers through the collector.
            }
        }
    })
}

/// A pulled-but-unread reply, kept for the next taker: `pull_replies` hands
/// back everything queued, and a reply behind the one this call needs must
/// never be dropped. The deadline only catches a lost reply: each reply is
/// real `git` work, and the gate runs this filter's repos in parallel.
pub(in crate::server) fn wait_for_worker_reply(
    conn: &ConnHandle,
    backlog: &mut VecDeque<DaemonMessage>,
) -> DaemonMessage {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        backlog.extend(conn.outbound.pull_replies());
        if let Some(reply) = backlog.pop_front() {
            return reply;
        }
        assert!(
            Instant::now() < deadline,
            "background request did not reply"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The queue key the dispatcher computes for a workspace — the resolved
/// root, in the same spelling `git()` receives.
pub(super) fn resolved_root(state: &ServerState, workspace_id: &str) -> String {
    state
        .sessions
        .workspace_cwd(workspace_id)
        .expect("workspace cwd")
        .to_string_lossy()
        .into_owned()
}

/// Hold the next `git` command in this workspace's directory. The gate sits
/// in the git runner itself, so it bites on whichever road runs the command.
pub(super) fn arm_git_gate(
    state: &ServerState,
    workspace_id: &str,
) -> (
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::SyncSender<()>,
) {
    let cwd = state
        .sessions
        .workspace_cwd(workspace_id)
        .expect("workspace cwd");
    crate::workspace_git_support::arm_git_command_gate(&cwd)
}

/// `(queued jobs, queued reads, waitlisted reads)` for one root — the
/// white-box view the bound and coalescing tests assert on.
pub(super) fn queue_shape(state: &ServerState, root: &str) -> (usize, usize, usize) {
    state.git_jobs.queue_shape(root)
}

/// Poll until the root's queue holds at least `len` jobs. An enqueue
/// barrier — two writes are dispatched from their own threads, and this
/// pins their order at the queue rather than at the scheduler — bounded,
/// and silent on a tree that has no queue: the shape assert after it is
/// the red there.
pub(super) fn wait_queue_len(state: &ServerState, root: &str, len: usize) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while queue_shape(state, root).0 < len && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A repository under `%TEMP%`, hard-requiring git: the ordering and bound
/// tests run real index and status commands against it.
pub(super) struct TestRepo {
    pub(super) root: PathBuf,
}

impl TestRepo {
    pub(super) fn new(label: &str) -> Self {
        let root = crate::test_dirs::test_temp_dir(&format!("devboule-{label}"));
        let repo = Self { root };
        repo.run(&["init", "--quiet"]);
        repo.run(&["config", "user.email", "test@devboule.local"]);
        repo.run(&["config", "user.name", "devboule test"]);
        repo.run(&["config", "core.autocrlf", "false"]);
        repo.run(&["config", "commit.gpgsign", "false"]);
        repo.write("base.txt", "base\n");
        repo.commit("base");
        repo
    }

    fn git(&self, arguments: &[&str]) -> std::io::Result<std::process::Output> {
        Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(arguments)
            .output()
    }

    pub(super) fn run(&self, arguments: &[&str]) {
        let output = self.git(arguments).expect("git could not be spawned");
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub(super) fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent directory");
        }
        std::fs::write(path, contents).expect("write");
    }

    pub(super) fn commit(&self, message: &str) {
        self.run(&["add", "-A"]);
        self.run(&["commit", "--quiet", "--message", message]);
    }

    pub(super) fn commit_subjects(&self) -> String {
        let output = self.git(&["log", "--format=%s"]).expect("git log");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
