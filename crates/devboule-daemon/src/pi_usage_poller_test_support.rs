//! Shared harness for the pi usage-poller tests: the poller is built the
//! way spawn builds it and reaches the reader only through the production
//! wiring; a fake pi answers the stats RPC from files each test rewrites,
//! can hold a reply until the test releases it, and logs every request
//! frame it receives (so a test can assert no request was sent, not just
//! that nothing published). The poll clock is the poller's own
//! deterministic tick, never slept out. The reader runs on its own thread
//! over the child's stdout, as production's does, so a tick's blocking
//! round trip is answered while the test waits in the tick.
//!
//! The watch harness (`pi_turn_watch_test_support.rs`) keeps its own fake:
//! a pure echo driven by manual feeds, where this one must answer RPCs and
//! run the reader concurrently. `drain()` follows the repo-wide per-file
//! convention; the broker comes from the pi watch support.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_protocol::{SessionEvent, SessionKind};
use serde_json::Value;

use super::super::{catalog_from_responses, send_json, PiCatalog, PiControl, PiKiller, PiReader};
use super::PiUsagePoller;
use crate::session::event_pull::ConnHandle;
use crate::session::permission_broker::PermissionBroker;
use crate::session::turn_watch::TurnWatch;
use crate::session::{ReaderDispatch, SessionRuntime};

/// The stats fake's half of the stdin protocol: `get_session_stats` is
/// answered from the JSON file the env names — re-read per request, so a
/// test restages the body between ticks; an unreadable file answers
/// `success:false`, the refusal shape. When the hold file exists, the
/// answer waits (writing its `<hold>.held` marker) until the test deletes
/// the hold. Every frame received is appended to the request log — or the
/// fake exits 42 on the first failed append — and every non-stats
/// frame is echoed verbatim, which is how a test feeds inbound frames
/// (`agent_start`, a `turn_end`) through the production reader. The
/// framing loop is the shared preamble; this only handles whole lines,
/// newline kept.
const STATS_PI_FRAMED: &str = r#"
const fs = require("fs");
function logFrame(frame) {
  try {
    fs.appendFileSync(process.env.DEVBOULE_FAKE_LOG_FILE, JSON.stringify({ type: frame.type }) + "\n");
  } catch (error) {
    const message = "fake pi cannot append to its request log: " + error.message + "\n";
    // writeSync, because process.exit drops an async stderr write.
    try { fs.writeSync(2, message); } catch (ignored) {}
    process.exit(42);
  }
}
function waitForRelease(file, go) {
  if (!fs.existsSync(file)) return go();
  setTimeout(() => waitForRelease(file, go), 5);
}
function reply(id, file) {
  let data = null;
  let success = false;
  try {
    data = JSON.parse(fs.readFileSync(file, "utf8"));
    success = true;
  } catch (error) {}
  const frame = { id, type: "response", command: "get_session_stats", success };
  if (success) frame.data = data;
  process.stdout.write(JSON.stringify(frame) + "\n");
}
function onFramedLine(line) {
  let frame;
  try {
    frame = JSON.parse(line);
  } catch (error) {
    return;
  }
  logFrame(frame);
  if (frame.type === "get_session_stats" && process.env.DEVBOULE_FAKE_STATS_FILE) {
    const hold = process.env.DEVBOULE_FAKE_HOLD_FILE;
    const go = () => reply(frame.id, process.env.DEVBOULE_FAKE_STATS_FILE);
    if (hold && fs.existsSync(hold)) {
      fs.writeFileSync(hold + ".held", "held");
      waitForRelease(hold, go);
    } else {
      go();
    }
  } else {
    process.stdout.write(line);
  }
}
"#;

pub(super) struct PiUsageHarness {
    child: Arc<Mutex<Child>>,
    pub(super) stdin: Arc<Mutex<Option<ChildStdin>>>,
    pub(super) control: Arc<PiControl>,
    pub(super) conn: Arc<ConnHandle>,
    pub(super) poller: Arc<PiUsagePoller>,
    /// Held, never touched: the watch's own thread is what judges the
    /// silence in the interference test, and dropping this would stop it.
    pub(super) _watch: Arc<TurnWatch>,
    pub(super) killer: PiKiller,
    pub(super) catalog: Arc<Mutex<PiCatalog>>,
    pub(super) stats_path: PathBuf,
    pub(super) hold_path: PathBuf,
    pub(super) log_path: PathBuf,
}

impl Drop for PiUsageHarness {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(self.dir());
    }
}

impl PiUsageHarness {
    fn dir(&self) -> PathBuf {
        self.stats_path
            .parent()
            .expect("stats file sits in its own temp dir")
            .to_path_buf()
    }
}

/// The catalog spawn hands the poller, built through the production
/// parser: one model `m`, current, declaring the window given.
pub(super) fn harness_catalog(window: Option<u64>) -> Arc<Mutex<PiCatalog>> {
    let mut model = serde_json::json!({"id": "m", "name": "M", "provider": "p"});
    if let Some(window) = window {
        model["contextWindow"] = window.into();
    }
    let state = serde_json::json!({"data": {"model": {"id": "m", "provider": "p"}}});
    let models = serde_json::json!({"data": {"models": [model]}});
    let levels = serde_json::json!({"data": {"levels": []}});
    Arc::new(Mutex::new(
        catalog_from_responses(&state, &models, &levels).expect("catalog"),
    ))
}

/// The production construction, shrunk to one harness: the session is
/// already attached, and the poller shares the control channel and the
/// catalog with the reader and the arbiter. `silence` is the stall
/// watchdog's bound, so the interference test can sit inside it.
fn harness_on(
    broker: &Arc<PermissionBroker>,
    catalog: Arc<Mutex<PiCatalog>>,
    runtime: Arc<SessionRuntime>,
    conn: Arc<ConnHandle>,
    silence: Duration,
) -> PiUsageHarness {
    let dir = crate::test_dirs::test_temp_dir("devboule-pi-usage");
    let stats_path = dir.join("stats.json");
    let hold_path = dir.join("hold");
    let log_path = dir.join("requests.log");
    let script = format!(
        "{}{}",
        crate::test_support::NODE_FRAMED_STDIN,
        STATS_PI_FRAMED
    );
    let child = Command::new("node")
        .arg("-e")
        .arg(script)
        .env("DEVBOULE_FAKE_STATS_FILE", &stats_path)
        .env("DEVBOULE_FAKE_HOLD_FILE", &hold_path)
        .env("DEVBOULE_FAKE_LOG_FILE", &log_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| {
            panic!(
                "node is required for the pi usage-poller tests: could not spawn `node` ({error})"
            )
        });
    let child = Arc::new(Mutex::new(child));
    let stdin = Arc::new(Mutex::new(Some(
        child.lock().expect("child").stdin.take().expect("stdin"),
    )));
    let stdout = child.lock().expect("child").stdout.take().expect("stdout");
    let next_id = Arc::new(AtomicU64::new(1));
    let cancelled = Arc::new(AtomicBool::new(false));
    let owed_late_end = Arc::new(super::super::pi_turn_watch::OwedTurnEnd::default());
    let control = Arc::new(PiControl::new(Arc::clone(&stdin), Arc::clone(&next_id)));
    let poller = PiUsagePoller::new(Arc::clone(&control), Arc::clone(&catalog));
    let watch = super::super::pi_turn_watch::pi_turn_watch(
        silence,
        Arc::clone(&stdin),
        Arc::clone(&next_id),
        Arc::clone(broker),
        Arc::clone(&cancelled),
        Arc::clone(&owed_late_end),
        Some(Arc::clone(&poller)),
    );
    let arbiter = Arc::new(
        super::super::pi_turn_arbiter::TurnArbiter::new(Some(Arc::clone(&watch)), owed_late_end)
            .with_usage_poller(Arc::clone(&poller)),
    );
    let killer = PiKiller {
        process: Arc::clone(&child),
        stdin: Arc::clone(&stdin),
        next_id: Arc::clone(&next_id),
        permission_broker: Arc::clone(broker),
        arbiter: Arc::clone(&arbiter),
        cancelled,
        extension_path: crate::test_dirs::test_temp_dir("devboule-pi-usage-ext")
            .join("extension.ts"),
        bridge_path: None,
    };
    let reader = PiReader::new(
        Vec::new(),
        SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: None,
            current_model_provider_id: None,
        },
        Arc::clone(broker),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::clone(&next_id),
        Arc::clone(&control),
        Arc::clone(&stdin),
        Arc::new(AtomicBool::new(true)),
    )
    .with_turn_arbiter(Arc::clone(&arbiter));
    spawn_reader(reader, stdout, Arc::clone(&runtime));
    PiUsageHarness {
        child,
        stdin,
        control,
        conn,
        poller,
        _watch: watch,
        killer,
        catalog,
        stats_path,
        hold_path,
        log_path,
    }
}

/// A harness on a live (journal-less) session, the way production is when
/// a prompt goes out, under the watch's 30 s test silence.
pub(super) fn harness(broker: &Arc<PermissionBroker>) -> PiUsageHarness {
    harness_with_catalog(broker, harness_catalog(Some(4000)))
}

/// The same, on a catalog the test composes — a model with no declared
/// window, for one.
pub(super) fn harness_with_catalog(
    broker: &Arc<PermissionBroker>,
    catalog: Arc<Mutex<PiCatalog>>,
) -> PiUsageHarness {
    attach_harness(broker, catalog, Duration::from_secs(30))
}

/// A harness whose stall watchdog sits on a short real silence, so the
/// interference test can wait it out in bounded real time — the one thing
/// the deterministic hooks cannot fake, because the test's subject IS what
/// restamps the clock.
pub(super) fn harness_with_silence(
    broker: &Arc<PermissionBroker>,
    silence: Duration,
) -> PiUsageHarness {
    attach_harness(broker, harness_catalog(Some(4000)), silence)
}

fn attach_harness(
    broker: &Arc<PermissionBroker>,
    catalog: Arc<Mutex<PiCatalog>>,
    silence: Duration,
) -> PiUsageHarness {
    let runtime = SessionRuntime::for_acp("s.pi.usage".to_string(), None, Arc::clone(broker));
    runtime.set_agent_kind(SessionKind::Pi);
    let conn = crate::test_support::attach_and_track(&runtime, "s.pi.usage");
    harness_on(broker, catalog, runtime, conn, silence)
}

/// A harness on a journaled session, so a test can assert the transcript
/// live AND as a restart replays it.
pub(super) fn harness_journaled(
    broker: &Arc<PermissionBroker>,
    journal: &Arc<crate::journal::Journal>,
    session_id: &str,
) -> PiUsageHarness {
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(journal)),
    ));
    runtime.set_agent_kind(SessionKind::Pi);
    let conn = crate::test_support::attach_and_track(&runtime, session_id);
    harness_on(
        broker,
        harness_catalog(Some(4000)),
        runtime,
        conn,
        Duration::from_secs(30),
    )
}

/// The reader loop, in miniature: the production `feed` over the child's
/// stdout on its own thread, `finish` at EOF — the session roads the same
/// teardown production takes. The runtime and its connection belong to the
/// test; the reader publishes into the same session the test drains.
fn spawn_reader(mut reader: PiReader, mut stdout: ChildStdout, runtime: Arc<SessionRuntime>) {
    let _ = std::thread::Builder::new()
        .name("pi-usage-test-reader".to_string())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(length) => {
                        if reader.feed(&buf[..length], &runtime).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            reader.finish(&runtime);
        });
}

/// A frame the test feeds inbound, through the fake's echo and the
/// production reader — not injected past any of it.
pub(super) fn feed_line(harness: &PiUsageHarness, value: Value) {
    send_json(&harness.stdin, &value, "Pi").expect("feed the frame");
}

/// Stage the body the next `get_session_stats` answer carries: the whole
/// reply `data`, so a test writes `contextUsage` exactly as pi would.
pub(super) fn stage_stats(harness: &PiUsageHarness, data: Value) {
    std::fs::write(&harness.stats_path, data.to_string()).expect("stage stats body");
}

/// Unstage the stats body: the next request is refused, the shape an old
/// binary's unknown command takes.
pub(super) fn unstage_stats(harness: &PiUsageHarness) {
    let _ = std::fs::remove_file(&harness.stats_path);
}

/// Hold the fake's next `get_session_stats` reply until [`release_hold`].
pub(super) fn stage_hold(harness: &PiUsageHarness) {
    std::fs::write(&harness.hold_path, "hold").expect("stage hold");
}

pub(super) fn release_hold(harness: &PiUsageHarness) {
    let _ = std::fs::remove_file(&harness.hold_path);
}

/// Wait, bounded, until the fake is holding a stats request — the proof
/// the request went out and the reply has not landed.
pub(super) fn wait_held(harness: &PiUsageHarness) {
    let held = harness.hold_path.with_extension("held");
    wait_for_existence(&held, "the fake to hold the reply");
}

fn wait_for_existence(path: &Path, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        if Instant::now() > deadline {
            panic!("timed out waiting for {label}");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The fake's append-failure exit code — the `process.exit` in
/// `STATS_PI_FRAMED`. Node documents 3 as an internal-failure code (an
/// internal JavaScript parse error), so the fake exits 42 instead.
const FAKE_LOG_EXIT: i32 = 42;

/// One sample of the fake's exit status: `None` while it still runs.
fn fake_exit(harness: &PiUsageHarness) -> Option<ExitStatus> {
    harness
        .child
        .lock()
        .expect("child")
        .try_wait()
        .expect("the fake's exit status")
}

/// Fail when the fake has died blocked on its request log: that exit code
/// is the one sign a writer which cannot append leaves behind. A fake
/// still running, killed or exited clean passes.
fn assert_fake_not_blocked(harness: &PiUsageHarness) {
    if let Some(status) = fake_exit(harness) {
        assert!(
            status.code() != Some(FAKE_LOG_EXIT),
            "the fake pi exited ({status}) because it could not append to the request log {}",
            harness.log_path.display()
        );
    }
}

/// Bounded wait for the fake to be gone: the kill road terminates the
/// process itself, so what follows samples a death instead of a fake not
/// yet scheduled to die.
pub(super) fn wait_dead(harness: &PiUsageHarness) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while fake_exit(harness).is_none() {
        if Instant::now() > deadline {
            panic!("timed out waiting for the fake pi to exit");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// How many `get_session_stats` requests the fake has received — the "no
/// RPC was sent" assertion a publication-only assertion cannot make.
/// Both halves are point samples: the exit is taken on either side of
/// the read, so a fake dead by a blocked log panics here instead of
/// counting short (a later death lands on the next call), and a log the
/// process cannot open panics on the read. The count's happens-before is
/// the caller's: wait for the round trip — the fake appends a frame
/// before it echoes or answers it.
pub(super) fn stats_requests(harness: &PiUsageHarness) -> usize {
    assert_fake_not_blocked(harness);
    let log = std::fs::read_to_string(&harness.log_path).unwrap_or_else(|error| {
        panic!(
            "read the fake request log {}: {error}",
            harness.log_path.display()
        )
    });
    // Sampled again for the one window the first sample cannot see: a
    // fake that dies blocked while the read runs.
    assert_fake_not_blocked(harness);
    log.lines()
        .filter(|line| line.contains(r#""type":"get_session_stats""#))
        .count()
}

/// Drain everything the connection currently holds.
pub(super) fn drain(conn: &ConnHandle) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    loop {
        let batch = conn.pull_events();
        if batch.is_empty() {
            return events;
        }
        for event in &batch {
            conn.event_sent(event);
        }
        events.extend(batch.into_iter().map(|pending| pending.envelope.event));
    }
}

/// Wait, bounded, for the reader thread (or the poller's tick, running on
/// the test thread) to produce a matching event. This waits on the child's
/// and the reader's own concurrency, never on the poll clock — ticks are
/// driven.
pub(super) fn wait_for(
    conn: &ConnHandle,
    mut seen: impl FnMut(&[SessionEvent]) -> bool,
    label: &str,
) -> Vec<SessionEvent> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut all = Vec::new();
    loop {
        let batch = drain(conn);
        let found = seen(&batch);
        all.extend(batch);
        if found {
            return all;
        }
        if Instant::now() > deadline {
            panic!("timed out waiting for {label}");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The `(model, used, max, live)` triples a pull carried, in order.
pub(super) fn context_usages(
    events: &[SessionEvent],
) -> Vec<(Option<String>, u64, Option<u64>, bool)> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ContextUsage {
                model_id,
                used_tokens,
                max_tokens,
                live,
            } => Some((model_id.clone(), *used_tokens, *max_tokens, *live)),
            _ => None,
        })
        .collect()
}

/// Wait, bounded, for the run wiring to move the window. The reader thread
/// performs the move; this only observes it.
pub(super) fn wait_for_window(harness: &PiUsageHarness, open: bool, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while harness.poller.window_open_for_test() != open {
        // The move arrives on what the fake echoed, so a fake blocked on
        // its log can never make it: name that instead of timing out.
        assert_fake_not_blocked(harness);
        if Instant::now() > deadline {
            panic!("timed out waiting for the window to be {open}: {label}");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// One benign inbound frame plus a bounded wait: the reader's first feed
/// binds the runtime the poller publishes through, and a tick before that
/// bind would publish nowhere.
pub(super) fn prime(harness: &PiUsageHarness) {
    feed_line(harness, serde_json::json!({"type": "message_update"}));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !harness.poller.runtime_bound_for_test() {
        // The fake echoes only what it appended, so a log it cannot write
        // never binds: name the exit instead of timing out below.
        assert_fake_not_blocked(harness);
        if Instant::now() > deadline {
            let dead = match fake_exit(harness) {
                Some(status) => format!("; the fake pi is dead ({status})"),
                None => String::new(),
            };
            panic!("timed out waiting for the reader to bind the runtime{dead}");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    drain(&harness.conn);
}

/// A `turn_end` pi's wire carries: model `m`, the usage the durable
/// reading is taken from, and the stop reason named.
pub(super) fn turn_end(usage_total: u64, stop_reason: &str) -> Value {
    serde_json::json!({
        "type": "turn_end",
        "message": {
            "role": "assistant",
            "content": [],
            "model": "m",
            "usage": {"totalTokens": usage_total},
            "stopReason": stop_reason,
        },
        "toolResults": [],
    })
}

/// Skip the test when `node` is absent, the suite-wide rule.
pub(super) fn node_skip() -> bool {
    crate::test_support::external_program_skip_reason("node").is_some()
}
