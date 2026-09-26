//! The idle-close timer's outputs: the notice the close leaves on the child's
//! own transcript and its order relative to the close, the dedicated envelope
//! the creator receives, and the sentence `devboule_send_message` answers with
//! when its target is one of the caller's own children, closed.

use super::session_idle_close_tests::{
    birth_row, idle_state, linked_child, linked_creator, shut_down,
};
use super::tests::{insert_live_agent_with_kind_and_writer, test_owner, RecordingWriter};
use super::*;
use devboule_protocol::NoticeSeverity;

#[test]
fn the_notice_is_written_before_the_teardown_and_the_creator_gets_the_envelope() {
    let (state, dir) = idle_state("notice");
    let registry = &state.sessions;
    let owner = test_owner("idle-notice-user", "idle-notice-client");
    let creator = "idle-notice-creator";
    let received = Arc::new(Mutex::new(Vec::new()));
    insert_live_agent_with_kind_and_writer(
        registry,
        creator,
        owner.clone(),
        SessionKind::Acp,
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    birth_row(registry, creator, &owner, None, None);
    linked_child(registry, "idle-notice-child", &owner, creator);
    let child_runtime = registry
        .child_view("idle-notice-child")
        .expect("live child")
        .1;
    let journal = registry.journal.clone().expect("journal");

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + Duration::from_secs(30 * 60)),
        1
    );

    // The notice is in the bytes the close left behind. Every normal replay
    // refuses a closed row by design (`replay_session`), so the test reads
    // the store's own events table — the child's journal, not a copy of it.
    journal.flush().expect("the notice is durable");
    let conn = rusqlite::Connection::open_with_flags(
        dir.join("journal.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open the journal the daemon just wrote");
    let mut stmt = conn
        .prepare("SELECT payload FROM events WHERE session_id = ?1 AND kind = 'agent_report'")
        .expect("one session's events");
    let payloads: Vec<Vec<u8>> = stmt
        .query_map(["idle-notice-child"], |row| row.get(0))
        .expect("the query runs")
        .collect::<Result<_, _>>()
        .expect("the rows");
    assert!(
        payloads
            .iter()
            .any(|payload| String::from_utf8_lossy(payload)
                .contains("closed: idle after 30 minutes")),
        "the closed child's transcript carries the notice"
    );
    assert!(
        journal
            .list()
            .expect("list")
            .iter()
            .all(|row| row.id != "idle-notice-child"),
        "the roster list drops the closed row, which is why the send's refusal has its own read"
    );
    // The notice was written before the teardown: a publish no longer lands
    // once the close has torn the stream down, so the one above can only
    // have come from the child's last open moment.
    assert!(
        !child_runtime.publish_daemon_event(SessionEvent::SessionNotice {
            text: "after the close".to_string(),
            severity: NoticeSeverity::Info,
        }),
        "a closed stream refuses a publish — which is what orders the notice before the teardown"
    );

    // The creator's own envelope, in its frame and its words.
    let written = String::from_utf8_lossy(&received.lock().expect("written")).into_owned();
    assert!(written.contains("kind: agent_idle_closed"), "{written}");
    assert!(
        written.contains("closed: idle after 30 minutes"),
        "{written}"
    );
    // The field the app's card parses and refuses to guess at: the summary
    // carries the same words, so only this line pins the wire form itself.
    assert!(written.contains("idleMinutes: 30"), "{written}");
    shut_down(&state, &dir);
}

#[test]
fn a_send_to_an_idle_closed_child_gets_the_sentence_and_nothing_reopens() {
    let (state, dir) = idle_state("send");
    let registry = &state.sessions;
    let owner = test_owner("idle-send-user", "idle-send-client");
    let creator = "idle-send-creator";
    linked_creator(registry, creator, &owner);
    linked_child(registry, "idle-send-child", &owner, creator);
    let journal = registry.journal.clone().expect("journal");

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + Duration::from_secs(30 * 60)),
        1
    );

    let sentence =
        "your child 'child' is closed: idle; closed sessions do not reopen — create a new one";
    assert_eq!(
        registry.closed_child_refusal(&owner, creator, "idle-send-child"),
        Some(sentence.to_string()),
        "the reason is known: this close recorded it"
    );
    assert_eq!(
        registry.closed_child_refusal(&owner, creator, "Agent"),
        Some(sentence.to_string()),
        "the title is a target the live lookup takes too"
    );
    // Nothing reopens: the row stays closed, no session comes back, and the
    // sentence is the same on the second ask as on the first.
    let rows = journal.list().expect("list");
    assert!(
        !rows.iter().any(|row| row.id == "idle-send-child"),
        "the closed row is out of the roster's list: {rows:?}"
    );
    assert!(!registry
        .inner
        .lock()
        .expect("registry")
        .contains_key("idle-send-child"));
    assert_eq!(
        registry.closed_child_refusal(&owner, creator, "idle-send-child"),
        Some(sentence.to_string())
    );
    // A caller that did not create the child, and a name that is nobody's
    // child, are told nothing about what exists.
    assert_eq!(
        registry.closed_child_refusal(&owner, "idle-send-other", "idle-send-child"),
        None
    );
    assert_eq!(
        registry.closed_child_refusal(&owner, creator, "idle-send-nobody"),
        None
    );
    shut_down(&state, &dir);
}

#[test]
fn a_child_closed_by_another_road_is_simply_closed() {
    let (state, dir) = idle_state("plain");
    let registry = &state.sessions;
    let owner = test_owner("idle-plain-user", "idle-plain-client");
    let creator = "idle-plain-creator";
    linked_creator(registry, creator, &owner);
    linked_child(registry, "idle-plain-child", &owner, creator);

    // Closed by the ordinary close rather than by idleness: the reason is
    // unknown here, and unknown is "closed", never "closed: idle".
    registry
        .close("idle-plain-child", &owner, &None)
        .expect("the child closes");
    assert_eq!(
        registry.closed_child_refusal(&owner, creator, "idle-plain-child"),
        Some(
            "your child 'child' is closed; closed sessions do not reopen — create a new one"
                .to_string()
        )
    );
    shut_down(&state, &dir);
}

/// The prompt route to the creator refuses at once when its broker has not
/// come up — the zero wait is what keeps the shared sweep thread off
/// `MCP_READY_TIMEOUT` — and the envelope's latch is spent with the publish.
/// The fact must still reach a human: it lands on the creator's own
/// transcript as a daemon notice, which needs no broker.
#[test]
fn a_creator_whose_broker_is_not_ready_hears_it_on_its_own_transcript() {
    let (state, dir) = idle_state("notice-unready");
    let registry = &state.sessions;
    let owner = test_owner("idle-unready-user", "idle-unready-client");
    let creator = "idle-unready-creator";
    let received = Arc::new(Mutex::new(Vec::new()));
    let creator_runtime = insert_live_agent_with_kind_and_writer(
        registry,
        creator,
        owner.clone(),
        SessionKind::Acp,
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    birth_row(registry, creator, &owner, None, None);
    // The creator hosts MCP and has never been served: the readiness gate
    // can only refuse, never wait.
    creator_runtime.require_mcp();
    linked_child(registry, "idle-unready-child", &owner, creator);

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + Duration::from_secs(30 * 60)),
        1
    );

    assert!(
        received.lock().expect("written").is_empty(),
        "no prompt reached the creator: the gate refused before any write"
    );
    let journal = registry.journal.clone().expect("journal");
    let replay = journal
        .replay(creator)
        .expect("the open creator still replays");
    assert!(
        replay.events.iter().any(|event| matches!(
            event,
            SessionEvent::SessionNotice { text, .. }
                if text.contains("closed: idle after 30 minutes")
        )),
        "the creator's own transcript carries the fact: {:#?}",
        replay.events
    );
    shut_down(&state, &dir);
}
