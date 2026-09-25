//! The turn status on the roster row: what each change pushes, and what a
//! non-change refuses to.
//!
//! The status exists because a queued message has to know when it may be sent,
//! and the attention raise cannot answer that: presence suppresses the raise for
//! the session a client of the same user is looking at, and a suppressed raise is
//! *dropped*, not delayed. These tests pin the difference — the turn facts a
//! session's own thread moves (`begin_turn` is what an accepted prompt calls,
//! `session_messaging.rs:1062`; publishing `AgentFinished` is what closes a turn)
//! each reach the roster for a session nobody is attached to *and* for one another
//! connection has focused — plus the shape the app drains on: a change pushes
//! once, a repeat pushes nothing.

use super::tests::{
    attach_live_agent_for_test, insert_live_agent, insert_live_agent_with_writer, test_owner,
    tmp_delete_registry, RecordingWriter,
};
use super::*;
use std::sync::Mutex;

fn row_activity(
    registry: &SessionRegistry,
    owner: &OwnerId,
    id: &str,
) -> Option<AgentActivityState> {
    registry
        .state_snapshots(owner)
        .into_iter()
        .find(|row| row.id == id)
        .and_then(|row| row.activity)
}

fn runtime_of(registry: &SessionRegistry, id: &str) -> Arc<SessionRuntime> {
    registry
        .inner
        .lock()
        .expect("map")
        .get(id)
        .expect("session")
        .runtime()
}

/// A sink that logs whose roster was pushed. Installed where a step's pushes are
/// the thing being counted, so an earlier step's push cannot be mistaken for it.
fn watch_pushes(registry: &SessionRegistry) -> Arc<Mutex<Vec<String>>> {
    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let fired = Arc::clone(&log);
    registry.set_transition_sink(Arc::new(move |pushed, _snapshots| {
        fired.lock().expect("push log").push(pushed.user.clone());
    }));
    log
}

#[test]
fn a_turn_opening_and_a_turn_closing_each_push_the_status_once() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("act-open", "c1");
    let id = "s.activity.1";
    insert_live_agent(&registry, id, owner.clone());
    assert_eq!(
        row_activity(&registry, &owner, id),
        Some(AgentActivityState::Idle),
        "a live session with no turn running is idle, and the row says so"
    );
    let runtime = runtime_of(&registry, id);
    // Read the roster first: the cache holds the idle, so a push has to move the
    // cached row rather than merely promise a rebuild.
    let log = watch_pushes(&registry);

    runtime.begin_turn();
    assert_eq!(
        row_activity(&registry, &owner, id),
        Some(AgentActivityState::Working),
        "an accepted prompt is a turn running"
    );
    assert_eq!(
        *log.lock().expect("push log"),
        vec![owner.user.clone()],
        "a turn starting pushes the row exactly once"
    );

    log.lock().expect("push log").clear();
    runtime.begin_turn();
    assert!(
        log.lock().expect("push log").is_empty(),
        "a turn that was already running is not a change"
    );

    log.lock().expect("push log").clear();
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert_eq!(
        row_activity(&registry, &owner, id),
        Some(AgentActivityState::Idle),
        "the turn that closed is on the row without anyone asking"
    );
    // The turn-end also raises attention, and that raise pushes on its own
    // account: two transitions for one event is what the daemon actually does,
    // and the app's edge rule is what makes the second one harmless — it reads
    // the row, sees the idle it has already acted on, and does not drain twice.
    let pushed = log.lock().expect("push log").len();
    assert!(
        pushed >= 1,
        "a turn finishing reaches the roster (observed {pushed} pushes)"
    );

    log.lock().expect("push log").clear();
    runtime.publish_agent_event(
        SessionEvent::AgentError {
            message: "a notification, not a turn fact".to_string(),
        },
        None,
    );
    assert!(
        row_activity(&registry, &owner, id) == Some(AgentActivityState::Idle),
        "the error leaves the status where it was"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_status_that_has_not_changed_is_never_a_reason_to_push() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("act-still", "c1");
    let id = "s.activity.5";
    insert_live_agent(&registry, id, owner.clone());
    let runtime = runtime_of(&registry, id);
    let log = watch_pushes(&registry);
    // A silent session first becomes roster-visible, then publishes the turn's
    // idle edge. A repeated finish at the same state owes no activity push.
    runtime.begin_turn();
    log.lock().expect("push log").clear();
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert_eq!(
        row_activity(&registry, &owner, id),
        Some(AgentActivityState::Idle),
        "the first close reached the row"
    );
    assert_eq!(
        *log.lock().expect("push log"),
        vec![owner.user.clone(), owner.user.clone()],
        "the state restoration and turn edge are published separately"
    );
    log.lock().expect("push log").clear();
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert_eq!(
        *log.lock().expect("push log"),
        Vec::<String>::new(),
        "a session already idle is not a change"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_accepted_prompt_publishes_activity_and_attention_separately() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("act-prompt", "c1");
    let id = "s.activity.prompt";
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&bytes))),
    );
    let conn = attach_live_agent_for_test(&runtime, id, 91);
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    assert!(
        runtime.attention().is_some(),
        "a standing raise is required"
    );
    let log = watch_pushes(&registry);

    registry
        .send_with_subscription(id, conn.id, "accepted prompt", &[], &[], &owner, &conn)
        .expect("the prompt is accepted");

    assert_eq!(runtime.activity(), AgentActivityState::Working);
    assert!(runtime.attention().is_none());
    assert_eq!(
        log.lock().expect("push log").len(),
        2,
        "the accepted prompt publishes its turn start and attention clear separately"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_status_pushes_while_another_connection_has_the_session_focused() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("act-focus", "c1");
    let watcher = test_owner("act-focus", "c2");
    let id = "s.activity.2";
    insert_live_agent(&registry, id, owner.clone());
    // A second connection of the same user, visible and looking at this session:
    // the state in which an attention raise is dropped.
    registry
        .set_presence(1, &watcher, Some(id.to_string()), true)
        .expect("presence recorded");
    let runtime = runtime_of(&registry, id);
    runtime.begin_turn();
    let log = watch_pushes(&registry);

    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );

    assert!(
        runtime.attention().is_none(),
        "precondition: the raise this turn-end wanted to make was suppressed"
    );
    assert_eq!(
        *log.lock().expect("push log"),
        vec![owner.user.clone()],
        "the status still reached the roster: what presence withholds is the \
         notification, never the row"
    );
    assert_eq!(
        row_activity(&registry, &owner, id),
        Some(AgentActivityState::Idle),
        "and the pushed row carries it"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_row_with_no_status_skips_the_field_and_reads_as_none() {
    // The wire shape, not the registry: an older peer sends no status, and the
    // absence has to survive the round trip as absence — never as idle.
    let mut value = serde_json::json!({
        "id": "s.activity.4",
        "workspaceId": null,
        "kind": "acp",
        "title": "s.activity.4",
        "state": {
            "type": "ended",
            "generation": 1,
            "code": 0,
            "integrity": { "kind": "complete" },
        },
        "elapsedMs": null,
    });
    let parsed: SessionStateSnapshot =
        serde_json::from_value(value.clone()).expect("a row with no status parses");
    assert_eq!(parsed.activity, None, "absence is a value, never idle");
    value["activity"] = serde_json::json!("working");
    let with_status: SessionStateSnapshot =
        serde_json::from_value(value).expect("a row with a status parses");
    assert_eq!(with_status.activity, Some(AgentActivityState::Working));
}
