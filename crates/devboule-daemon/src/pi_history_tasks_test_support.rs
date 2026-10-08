//! Shared harness for the pi history-task tests: a fake pi process driven
//! over the framed stdin protocol, a reader fed from its stdout, and the
//! runtime and journal plumbing both history-task test files read results
//! from.

use devboule_protocol::{AgentTaskItem, AgentTaskStatus, SessionEvent};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::super::{PiControl, PiReader};
use crate::journal::Journal;
use crate::session::event_pull::ConnHandle;
use crate::session::permission_broker::PermissionBroker;
use crate::session::{ReaderDispatch, SessionRuntime};

pub(super) fn fake_pi(
    script: &str,
) -> (
    std::process::Child,
    Arc<Mutex<Option<std::process::ChildStdin>>>,
    std::io::BufReader<std::process::ChildStdout>,
) {
    let mut child = std::process::Command::new("node")
        .args(["-e", script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| {
            panic!("node is not runnable here: {error}");
        });
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    (child, stdin, stdout)
}

pub(super) fn reader_for(
    control: Arc<PiControl>,
    stdin: &Arc<Mutex<Option<std::process::ChildStdin>>>,
) -> PiReader {
    PiReader::new(
        Vec::new(),
        SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: None,
            current_model_provider_id: None,
        },
        PermissionBroker::for_test(Arc::new(|_, _| Ok(()))),
        Arc::new(Mutex::new(std::collections::HashMap::new())),
        Arc::new(AtomicU64::new(1)),
        control,
        Arc::clone(stdin),
        Arc::new(std::sync::atomic::AtomicBool::new(true)),
    )
}

/// The empty first feed is `reader_loop`'s own start — the pass arms there.
pub(super) fn feed_reader(
    mut reader: PiReader,
    mut stdout: std::io::BufReader<std::process::ChildStdout>,
    runtime: Arc<SessionRuntime>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let _ = reader.feed(&[], &runtime);
        let mut line = String::new();
        loop {
            line.clear();
            match std::io::BufRead::read_line(&mut stdout, &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let _ = reader.feed(line.as_bytes(), &runtime);
        }
    })
}

pub(super) fn attached_runtime(
    session_id: &str,
    journal: Option<Arc<Journal>>,
) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let runtime = SessionRuntime::for_acp(session_id.to_string(), journal, broker);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    (runtime, conn)
}

pub(super) fn pull_until(
    conn: &ConnHandle,
    wanted: impl Fn(&SessionEvent) -> bool,
    within: Duration,
) -> Vec<SessionEvent> {
    let deadline = Instant::now() + within;
    let mut events: Vec<SessionEvent> = Vec::new();
    loop {
        for pending in conn.pull_events() {
            conn.event_sent(&pending);
            events.push(pending.envelope.event);
        }
        if events.iter().any(&wanted) {
            return events;
        }
        assert!(
            Instant::now() < deadline,
            "the awaited event never arrived: {events:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The publish enqueues the event before it appends the row, so the journal
/// read retries rather than racing the commit.
pub(super) fn rows_after_the_snapshot(
    journal: &Journal,
    session_id: &str,
) -> Vec<crate::journal::EventRecord> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        journal.flush().expect("flush");
        let page = journal
            .replay_agent_page(session_id, 1, 0, 0, u64::MAX, 100)
            .expect("rows");
        if page.records.iter().any(|record| {
            matches!(
                serde_json::from_slice::<SessionEvent>(&record.payload),
                Ok(SessionEvent::AgentTasks { .. })
            )
        }) {
            return page.records;
        }
        assert!(
            Instant::now() < deadline,
            "the restored snapshot never reached the journal"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn restored_tasks(events: &[SessionEvent]) -> Vec<Vec<AgentTaskItem>> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentTasks { items } => Some(items.clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn item(id: &str, text: &str, status: AgentTaskStatus) -> AgentTaskItem {
    AgentTaskItem {
        id: Some(id.to_string()),
        text: text.to_string(),
        status,
        active_form: None,
    }
}
