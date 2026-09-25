//! The shared out-of-band route: record once, start no turn, skip attachments.

use super::tests::{attach_live_agent_for_test, test_owner, tmp_delete_registry, RecordingWriter};
use super::*;
use devboule_protocol::NoticeSeverity;
use std::sync::atomic::{AtomicUsize, Ordering};

struct CommandHandler {
    command: &'static str,
    checks: Arc<Mutex<Vec<String>>>,
    runs: Arc<AtomicUsize>,
}

impl CommandHandler {
    fn new(command: &'static str) -> (Self, Arc<Mutex<Vec<String>>>, Arc<AtomicUsize>) {
        let checks = Arc::new(Mutex::new(Vec::new()));
        let runs = Arc::new(AtomicUsize::new(0));
        (
            Self {
                command,
                checks: Arc::clone(&checks),
                runs: Arc::clone(&runs),
            },
            checks,
            runs,
        )
    }
}

impl OutOfBandCommands for CommandHandler {
    fn handles_out_of_band(&self, text: &str) -> bool {
        self.checks
            .lock()
            .expect("the checks")
            .push(text.to_string());
        text == self.command
    }

    fn run_out_of_band(&self, _text: &str, runtime: &Arc<SessionRuntime>) {
        self.runs.fetch_add(1, Ordering::Relaxed);
        let _ = runtime.publish_agent_event(
            SessionEvent::SessionNotice {
                text: "Goal cleared.".to_string(),
                severity: NoticeSeverity::Info,
            },
            None,
        );
    }
}

type Doubled = (
    Arc<SessionRuntime>,
    Arc<Mutex<Vec<String>>>,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<u8>>>,
);

fn session_with_command_handler(
    registry: &SessionRegistry,
    id: &str,
    owner: &OwnerId,
    command: &'static str,
) -> Doubled {
    let received = Arc::new(Mutex::new(Vec::new()));
    let (handler, checks, runs) = CommandHandler::new(command);
    let runtime = super::tests::insert_live_agent_with_out_of_band(
        registry,
        id,
        owner.clone(),
        SessionKind::Claude,
        Box::new(RecordingWriter(Arc::clone(&received))),
        Some(Arc::new(handler)),
    );
    (runtime, checks, runs, received)
}

#[test]
fn a_handled_command_is_recorded_once_and_starts_no_turn() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-oob", "process-oob");
    let session_id = "oob-command";
    let (runtime, checks, runs, received) =
        session_with_command_handler(&registry, session_id, &owner, "/goal clear");
    let conn = attach_live_agent_for_test(&runtime, session_id, 91);

    registry
        .send_with_subscription(session_id, 91, "/goal clear", &[], &[], &owner, &conn)
        .expect("the command is accepted");

    assert_eq!(checks.lock().expect("checks").as_slice(), ["/goal clear"]);
    assert_eq!(runs.load(Ordering::Relaxed), 1, "the handler ran once");
    assert!(received.lock().expect("writer").is_empty());
    assert!(!runtime.is_turn_active(runtime.turn_counter()));
    let events: Vec<SessionEvent> = conn
        .pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == "/goal clear"))
            .count(),
        1,
        "the shared seam owns exactly one transcript write"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::SessionNotice { text, severity }
            if text == "Goal cleared." && severity == &NoticeSeverity::Info
    )));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unclaimed_text_falls_through_as_an_ordinary_prompt() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-oob-passthrough", "process-oob-passthrough");
    let session_id = "oob-passthrough";
    let (runtime, checks, runs, received) =
        session_with_command_handler(&registry, session_id, &owner, "/goal clear");
    let conn = attach_live_agent_for_test(&runtime, session_id, 92);

    registry
        .send_with_subscription(session_id, 92, "/model gpt-5.5", &[], &[], &owner, &conn)
        .expect("the prompt is accepted");

    assert_eq!(
        checks.lock().expect("checks").as_slice(),
        ["/model gpt-5.5"]
    );
    assert_eq!(runs.load(Ordering::Relaxed), 0);
    assert_eq!(
        received.lock().expect("writer").as_slice(),
        b"/model gpt-5.5"
    );
    assert!(runtime.is_turn_active(runtime.turn_counter()));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_attachment_bypasses_the_out_of_band_route() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-oob-att", "process-oob-att");
    let session_id = "oob-attachment";
    let (runtime, checks, runs, received) =
        session_with_command_handler(&registry, session_id, &owner, "/goal clear");
    let conn = attach_live_agent_for_test(&runtime, session_id, 93);
    let image = crate::raster_metadata::clean_png(0x31);

    registry
        .send_with_subscription(
            session_id,
            93,
            "/goal clear",
            &[super::tests::attachment("photo.png", "image/png", &image)],
            &[],
            &owner,
            &conn,
        )
        .expect("an attachment send is accepted");

    assert!(checks.lock().expect("checks").is_empty());
    assert_eq!(runs.load(Ordering::Relaxed), 0);
    assert!(!received.lock().expect("writer").is_empty());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_out_of_band_command_settles_the_senders_optimistic_turn() {
    // The sender opens its optimistic turn on every send and only an
    // `agent_finished` closes it — but this door starts no turn, so without a
    // finish here the surface stays working forever, through every outcome.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-oob-settle", "process-oob-settle");
    let session_id = "oob-settle";
    let (runtime, _checks, _runs, _received) =
        session_with_command_handler(&registry, session_id, &owner, "/goal clear");
    let conn = attach_live_agent_for_test(&runtime, session_id, 94);

    registry
        .send_with_subscription(session_id, 94, "/goal clear", &[], &[], &owner, &conn)
        .expect("the command is accepted");

    let finishes: Vec<SessionEvent> = conn
        .pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .filter(|event| matches!(event, SessionEvent::AgentFinished { .. }))
        .collect();
    assert_eq!(
        finishes.len(),
        1,
        "one synthetic finish settles the optimistic turn the sender opened"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_out_of_band_command_leaves_a_live_turns_finish_alone() {
    // The synthetic finish belongs to the sender's optimistic turn only: with
    // a real turn running, its own finish settles the surface, and nothing
    // here may pre-empt it.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-oob-liveturn", "process-oob-liveturn");
    let session_id = "oob-liveturn";
    let (runtime, _checks, _runs, _received) =
        session_with_command_handler(&registry, session_id, &owner, "/goal clear");
    let conn = attach_live_agent_for_test(&runtime, session_id, 95);
    runtime.begin_turn();

    registry
        .send_with_subscription(session_id, 95, "/goal clear", &[], &[], &owner, &conn)
        .expect("the command is accepted");

    assert!(
        conn.pull_events()
            .into_iter()
            .all(|event| !matches!(event.envelope.event, SessionEvent::AgentFinished { .. })),
        "a live turn keeps its own finish"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
