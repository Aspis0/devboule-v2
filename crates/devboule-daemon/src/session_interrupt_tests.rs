//! `ActiveTurnBehavior::Interrupt`: with a running turn the daemon stops it,
//! waits for it to end, leaves the notice the transcript AND the journal
//! both record, and only then writes the text as a new turn; with no turn
//! running it is a plain send. Written first, per the Q1 brief.

use super::tests::{
    attach_live_agent_for_test, drain, insert_live_agent_with_writer, install_end_turn_killer,
    test_owner, tmp_delete_registry, RecordingWriter,
};
use super::{
    new_session_record, ActiveTurnBehavior, Arc, ConnHandle, Journal, Mutex, OwnerId, SessionEvent,
    SessionKind, SessionRegistry, SessionRuntime, WireError,
};
use devboule_protocol::NoticeSeverity;
use std::sync::atomic::{AtomicU64, Ordering};

const SESSION: &str = "s.q.int";
const NOTICE: &str = "The running turn was interrupted by the user's message.";

struct Harness {
    registry: SessionRegistry,
    owner: OwnerId,
    conn: Arc<ConnHandle>,
    runtime: Arc<SessionRuntime>,
    journal: Arc<Journal>,
    bytes: Arc<Mutex<Vec<u8>>>,
    interrupts: Arc<AtomicU64>,
    text_present: Arc<Mutex<Option<bool>>>,
}

fn harness() -> Harness {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    // The row this harness never writes by inserting a live entry: without
    // it the journal refuses every append for the id, and there would be
    // nothing for the replay half of the notice's record to read.
    journal
        .create_session(new_session_record(
            SESSION,
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("journal row");
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&bytes))),
    );
    let conn = attach_live_agent_for_test(&runtime, SESSION, 1);
    let probe = install_end_turn_killer(&registry, SESSION, &runtime, &bytes);
    Harness {
        registry,
        owner,
        conn,
        runtime,
        journal,
        bytes,
        interrupts: probe.interrupts,
        text_present: probe.text_present_at_interrupt,
    }
}

impl Harness {
    fn send_interrupt(&self, text: &str) -> Result<(), WireError> {
        self.registry.send_with_subscription_behavior(
            SESSION,
            self.conn.id,
            text,
            &[],
            &[],
            &self.owner,
            &self.conn,
            Some(ActiveTurnBehavior::Interrupt),
        )
    }

    fn written(&self) -> String {
        String::from_utf8_lossy(&self.bytes.lock().expect("writer")).to_string()
    }
}

#[test]
fn interrupt_stops_the_running_turn_waits_and_only_then_writes_the_text() {
    let h = harness();
    h.runtime.begin_turn();

    h.send_interrupt("the interrupting message").expect("send");

    assert_eq!(
        h.interrupts.load(Ordering::Acquire),
        1,
        "the running turn was interrupted"
    );
    assert_eq!(
        *h.text_present.lock().expect("record"),
        Some(false),
        "the writer held nothing when the turn ended — the send waited for it"
    );
    assert!(
        h.written().contains("the interrupting message"),
        "the text went as a new turn after the wait"
    );
    assert!(
        h.runtime.is_turn_active(h.runtime.turn_counter()),
        "the text began a new turn"
    );

    // The transcript's half of the record.
    let notice = drain(&h.conn)
        .into_iter()
        .find_map(|event| match event {
            SessionEvent::SessionNotice { text, severity } => Some((text, severity)),
            _ => None,
        })
        .expect("the interruption leaves a notice");
    assert_eq!(notice.0, NOTICE);
    assert_eq!(notice.1, NoticeSeverity::Info);

    // The journal's half: `publish_session_notice` writes the agent-report
    // row, so a replay — what any later attach reads — carries the same
    // sentence.
    let replay = h.journal.replay(SESSION).expect("journal replay");
    assert!(
        replay.events.iter().any(|event| matches!(
            event,
            SessionEvent::SessionNotice { text, .. } if text == NOTICE
        )),
        "the notice is journalled, not just pushed"
    );
}

#[test]
fn interrupt_with_no_turn_running_is_a_plain_send() {
    let h = harness();

    h.send_interrupt("no turn to interrupt").expect("send");

    assert_eq!(
        h.interrupts.load(Ordering::Acquire),
        0,
        "nothing was running"
    );
    assert!(h.written().contains("no turn to interrupt"));
    assert!(
        !drain(&h.conn).iter().any(|event| matches!(
            event,
            SessionEvent::SessionNotice { text, .. } if text == NOTICE
        )),
        "no interruption, no notice: absence is not invented"
    );
}

#[test]
fn send_now_interrupts_the_running_turn_the_queue_was_holding() {
    let h = harness();
    h.runtime.begin_turn();
    h.registry
        .queue_add(SESSION, "queued and urgent", &[], &[], &h.owner, &h.conn)
        .expect("add");
    assert_eq!(
        h.registry.queue_items_for_test(SESSION).len(),
        1,
        "the running turn holds the queue"
    );
    let item_id = h.registry.queue_items_for_test(SESSION)[0].item_id.clone();

    h.registry
        .queue_send_now(SESSION, h.conn.id, &item_id, &h.owner, &h.conn)
        .expect("send-now");

    assert_eq!(
        h.interrupts.load(Ordering::Acquire),
        1,
        "send-now is an interrupt"
    );
    assert_eq!(
        *h.text_present.lock().expect("record"),
        Some(false),
        "queued text also waits for the interrupted turn to end"
    );
    assert!(h.written().contains("queued and urgent"));
    assert!(h.registry.queue_items_for_test(SESSION).is_empty());
}

/// Fix pass 1, P2-3: the interrupt must target the turn it armed against.
/// The seam ends that turn naturally — and starts the next one — between the
/// arm and the kill: the killer must not be called at all, the next turn
/// survives, and the text goes nowhere near it.
#[test]
fn a_natural_end_stops_the_interrupt_from_hitting_the_next_turn() {
    let h = harness();
    h.runtime.begin_turn(); // the turn this send will target
    {
        let runtime = Arc::clone(&h.runtime);
        h.registry.set_interrupt_before_kill_hook(Arc::new(move || {
            runtime.publish_agent_event(
                SessionEvent::AgentFinished {
                    stop_reason: "natural".to_string(),
                    model_id: None,
                    usage: None,
                },
                None,
            );
            runtime.begin_turn(); // whatever starts next must not be interrupted
        }));
    }

    let error = h
        .send_interrupt("after the race")
        .expect_err("another turn is running; this send must not enter it");
    assert!(
        error.message.contains("a turn began"),
        "the refusal names the race: {}",
        error.message
    );
    assert_eq!(
        h.interrupts.load(Ordering::Acquire),
        0,
        "the interrupt targets the turn it saw, or nothing"
    );
    assert!(
        h.runtime.is_turn_active(h.runtime.turn_counter()),
        "the next turn survives the old turn's interrupt"
    );
    assert!(!h.written().contains("after the race"));
    assert!(
        !drain(&h.conn).iter().any(|event| matches!(
            event,
            SessionEvent::SessionNotice { text, .. } if text == NOTICE
        )),
        "nothing was interrupted, so no interruption is announced"
    );
}
