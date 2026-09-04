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

fn daemon_bin() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_devboule_daemon") {
        return PathBuf::from(path);
    }
    target_bin("devboule-daemon.exe")
}

fn stub_bin() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_devboule_agent_stub") {
        return PathBuf::from(path);
    }
    target_bin("devboule-agent-stub.exe")
}

fn target_bin(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("target");
    path.push(if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    });
    path.push(name);
    path
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

    drop(client);
    harness.kill_daemon();

    let journal_path = harness.paths.journal_file();
    let deadline = Instant::now() + Duration::from_secs(3);
    let replay = loop {
        match Journal::open(&journal_path).and_then(|journal| journal.replay(&session.id, 0)) {
            Ok(replay)
                if replay.events.iter().any(|event| {
                    matches!(event, SessionEvent::AgentReported { agent, .. } if agent == "stub")
                }) =>
            {
                break replay;
            }
            Ok(_) | Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(replay) => panic!("journal replay missing announcement: {:?}", replay.events),
            Err(error) => panic!("journal replay failed: {error}"),
        }
    };
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
