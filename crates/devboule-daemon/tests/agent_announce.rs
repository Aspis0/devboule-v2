//! End-to-end announcement channel: a real PTY child reads injected env,
//! reopens the named pipe, and reports itself. No user CLI configs are touched.

#![cfg(windows)]

use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_daemon::{
    connect, current_user_sid, spawn_daemon, write_test_pty_command, DaemonClient, EventHandler,
    Journal, PtyCommand, RuntimePaths,
};
use devboule_protocol::{AgentActivityState, ClientHello, OwnerId, SessionEvent, SessionKind};

static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Point the daemons these tests spawn at the **file** secret store, rooted in
/// that daemon's own temp runtime dir, so the identity it writes dies with the
/// directory.
///
/// The daemon binary is a production build: with no `DEVBOULE_SECRET_STORE` it
/// selects the OS credential store and writes a `noise-static-<runtime dir
/// hash>` entry that nothing ever deletes (measured: 66 entries from this file
/// in one suite run, and enough of them make `CredWrite` fail with Windows
/// error 8 -- see `reports/remote-agents/keyring-test-leak-fix-report.md`).
/// Every spawn in this file passes through `daemon_bin()`, so the call lives
/// there.
fn file_secret_store() {
    // Set once, before the first spawn: the tests run in parallel threads, so a
    // process-wide write per call would race them.
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| std::env::set_var("DEVBOULE_SECRET_STORE", "file"));
}

fn daemon_bin() -> PathBuf {
    // Cargo names this env var after the bin verbatim (dashes included); an
    // underscore lookup never matches. A stale-binary fallback would
    // silently run "the past" and report green, so refuse to guess.
    file_secret_store();

    if let Ok(path) = std::env::var("CARGO_BIN_EXE_devboule-daemon") {
        return PathBuf::from(path);
    }
    if let Some(path) = option_env!("CARGO_BIN_EXE_devboule-daemon") {
        return PathBuf::from(path);
    }
    panic!(
        "CARGO_BIN_EXE_devboule-daemon was not provided by Cargo; refusing to guess a target directory binary (a stale one would test the past)"
    );
}

fn stub_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_devboule-agent-stub") {
        return PathBuf::from(path);
    }
    if let Some(path) = option_env!("CARGO_BIN_EXE_devboule-agent-stub") {
        return PathBuf::from(path);
    }
    panic!(
        "CARGO_BIN_EXE_devboule-agent-stub was not provided by Cargo; refusing to guess a target directory binary (a stale one would test the past)"
    );
}

fn unique_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "devboule announce {}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("runtime directory with spaces");
    dir
}

fn hello(name: &str) -> ClientHello {
    let sid = current_user_sid().expect("current user SID");
    ClientHello::m3a(
        OwnerId::new(sid, format!("announce-{name}-{}", std::process::id())).expect("owner"),
        "devboule-announce-test",
    )
}

struct Harness {
    dir: PathBuf,
    paths: RuntimePaths,
    child: Option<Child>,
}

impl Harness {
    fn spawn() -> Self {
        let dir = unique_dir();
        let paths = RuntimePaths::from_dir(&dir);
        let child = spawn_daemon(&daemon_bin(), &paths).expect("spawn daemon");
        let harness = Self {
            dir,
            paths,
            child: Some(child),
        };
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if connect(&harness.paths, hello("wait")).is_ok() {
                return harness;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("daemon did not start");
    }

    fn client(&self) -> DaemonClient {
        connect(&self.paths, hello("client")).expect("connect")
    }

    fn kill_daemon(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.kill_daemon();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn wait_for<F>(events: &Mutex<Vec<SessionEvent>>, timeout: Duration, predicate: F)
where
    F: Fn(&[SessionEvent]) -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate(&events.lock().expect("events lock")) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "timed out waiting for announcement: {:?}",
        events.lock().unwrap()
    );
}

#[test]
fn pty_stub_announces_over_the_named_pipe() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut harness = Harness::spawn();
    write_test_pty_command(
        &harness.paths,
        &PtyCommand::new(
            stub_bin().to_string_lossy().into_owned(),
            Vec::<String>::new(),
            std::env::current_dir().expect("cwd"),
            Vec::new(),
        ),
    )
    .expect("queue stub as the PTY child");

    let client = harness.client();
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    let events = Arc::new(Mutex::new(Vec::new()));
    let handler: EventHandler = {
        let events = Arc::clone(&events);
        Arc::new(move |envelope| {
            events.lock().expect("events").push(envelope.event);
        })
    };
    client
        .session_attach(&session.id, None, handler)
        .expect("attach");

    wait_for(&events, Duration::from_secs(8), |events| {
        let saw_env = events.iter().any(|event| match event {
            SessionEvent::Output { data, .. } => data.contains("DEVBOULE_ENV=1"),
            _ => false,
        });
        let saw_report = events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::AgentReported {
                    agent,
                    state: AgentActivityState::Working,
                    report_seq: Some(1),
                    agent_session_id: Some(id),
                    ..
                } if agent == "stub" && id == "stub-session"
            )
        });
        saw_env && saw_report
    });

    // The broadcast precedes the async commit, so wait for the row before the kill;
    // read-only because `Journal::open` runs recovery and would mark this live session interrupted.
    let journal_path = harness.paths.journal_file();
    let reader = rusqlite::Connection::open_with_flags(
        &journal_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap_or_else(|error| panic!("live journal read open failed: {error}"));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let reports = reader
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id = ?1 AND kind = 'agent_report'",
                [&session.id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or_else(|error| panic!("live journal read failed: {error}"));
        if reports >= 1 {
            break;
        }
        if Instant::now() >= deadline {
            panic!("live journal never committed the announcement: {reports} rows");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(reader);

    drop(client);
    harness.kill_daemon();

    // A transaction committed before the kill survives it, so one cold
    // replay is deterministic and needs no retry loop.
    let replay = Journal::open(&journal_path)
        .unwrap_or_else(|error| panic!("cold journal open failed: {error}"))
        .replay(&session.id)
        .unwrap_or_else(|error| panic!("cold journal replay failed: {error}"));
    assert!(
        replay.events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentReported {
                source,
                agent,
                state: AgentActivityState::Working,
                report_seq: Some(1),
                agent_session_id: Some(id),
                session_start_source: Some(start),
                ..
            } if source == "devboule:stub"
                && agent == "stub"
                && id == "stub-session"
                && start == "startup"
        )),
        "replayed events: {:?}",
        replay.events
    );
}

fn announced(events: &[SessionEvent], identity: &str, seq: u64) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            SessionEvent::AgentReported {
                agent,
                report_seq: Some(reported),
                agent_session_id: Some(id),
                ..
            } if agent == "stub" && id == identity && *reported == seq
        )
    })
}

fn queue_stub_pty(harness: &Harness) {
    write_test_pty_command(
        &harness.paths,
        &PtyCommand::new(
            stub_bin().to_string_lossy().into_owned(),
            Vec::<String>::new(),
            std::env::current_dir().expect("cwd"),
            Vec::new(),
        ),
    )
    .expect("queue stub as the PTY child");
}

/// One stub life over the real pipe: its identity, its seq list and its
/// start source come from the environment, the defaults reproducing the
/// single `startup` + seq 1 report the original test asserts.
fn spawn_stub_life(
    harness: &Harness,
    session_id: &str,
    identity: &str,
    seqs: &str,
    start: &str,
) -> std::process::Child {
    std::process::Command::new(stub_bin())
        .env("DEVBOULE_ENV", "1")
        .env("DEVBOULE_SESSION_ID", session_id)
        .env("DEVBOULE_SOCKET_PATH", &harness.paths.pipe_name)
        .env("DEVBOULE_AGENT_STUB_SESSION_ID", identity)
        .env("DEVBOULE_AGENT_STUB_SEQS", seqs)
        .env("DEVBOULE_AGENT_STUB_START", start)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("stub life")
}

fn attached_terminal(harness: &Harness) -> (DaemonClient, String, Arc<Mutex<Vec<SessionEvent>>>) {
    let client = harness.client();
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    let events = Arc::new(Mutex::new(Vec::new()));
    let handler: EventHandler = {
        let events = Arc::clone(&events);
        Arc::new(move |envelope| {
            events.lock().expect("events").push(envelope.event);
        })
    };
    client
        .session_attach(&session.id, None, handler)
        .expect("attach");
    (client, session.id, events)
}

fn reap(mut life: std::process::Child) {
    let _ = life.kill();
    let _ = life.wait();
}

/// The restart shape end to end: in a live Terminal session life A counts
/// to seq 5, then life B — a fresh process, a fresh identity, its own
/// `startup` — announces seq 1, and the daemon must accept the 5 → 1 fall
/// that a per-source gate rejects.
#[test]
fn a_restarted_stub_announces_its_own_first_seq() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut harness = Harness::spawn();
    queue_stub_pty(&harness);
    let (client, session_id, events) = attached_terminal(&harness);

    let first = spawn_stub_life(&harness, &session_id, "restart-a", "5", "startup");
    wait_for(&events, Duration::from_secs(8), |events| {
        announced(events, "restart-a", 5)
    });

    let second = spawn_stub_life(&harness, &session_id, "restart-b", "1", "startup");
    wait_for(&events, Duration::from_secs(8), |events| {
        announced(events, "restart-b", 1)
    });
    reap(second);
    reap(first);

    drop(client);
    harness.kill_daemon();
}

/// The resume shape end to end: life A counts to seq 7, then a stub with
/// the SAME identity announces `resume` + seq 1 — a fresh process over a
/// kept conversation — and the daemon must reset that key so seq 1 is
/// accepted instead of gated by the previous process's 7.
#[test]
fn a_resumed_stub_same_identity_announces_from_one() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut harness = Harness::spawn();
    queue_stub_pty(&harness);
    let (client, session_id, events) = attached_terminal(&harness);

    let first = spawn_stub_life(&harness, &session_id, "resume-x", "7", "startup");
    wait_for(&events, Duration::from_secs(8), |events| {
        announced(events, "resume-x", 7)
    });

    let second = spawn_stub_life(&harness, &session_id, "resume-x", "1", "resume");
    wait_for(&events, Duration::from_secs(8), |events| {
        announced(events, "resume-x", 1)
    });
    reap(second);
    reap(first);

    drop(client);
    harness.kill_daemon();
}
