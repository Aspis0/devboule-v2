//! The daemon-owned `/goal` intercept: what the text means, and every refusal.
//! One phrase for the file — parsing and refusals — beside dispatch
//! (`session_goal_dispatch_tests.rs`), the durable half
//! (`session_goal_journal_tests.rs`), and the menu
//! (`session_goal_menu_tests.rs`).

use std::sync::{Arc, Mutex};

use devboule_protocol::{AttachmentReference, PromptAttachment, SessionKind};

use super::super::tests::{
    attach_live_agent_for_test, insert_live_agent_with_out_of_band, insert_terminal_with_writer,
    insert_transcript, insert_transcript_with_kind, test_owner, tmp_delete_registry,
    RecordingWriter,
};
use super::super::{ConnHandle, OwnerId, SessionRegistry};
use super::goal_test_support::{
    goal_text_of, live_agent, notices_of, png_attachment, pulled_events, runtime_without_journal,
    stored_reference, ClaimingOutOfBand,
};
use super::{intercept_goal, is_reserved_goal_command, GoalAction, MAX_GOAL_CHARS};

#[test]
fn text_without_a_slash_command_is_not_ours() {
    let runtime = runtime_without_journal();
    for text in [
        "hello",
        "",
        "   ",
        "/model gpt-5.5",
        "a /goal b",
        "/goalx y",
        "//goal x",
    ] {
        assert!(
            intercept_goal(text, &SessionKind::Claude, &runtime, None)
                .expect("parse")
                .is_none(),
            "{text:?} passes through"
        );
    }
}

#[test]
fn a_terminal_refuses_the_goal_with_one_line() {
    let runtime = runtime_without_journal();
    let error = intercept_goal("/goal ship it", &SessionKind::Terminal, &runtime, None)
        .expect_err("a terminal has no agent to carry a goal");
    assert_eq!(error.message, "This session does not accept a goal.");
}

#[test]
fn bare_goal_reads_back_the_current_goal_and_the_usage() {
    let runtime = runtime_without_journal();
    let conn = attach_live_agent_for_test(&runtime, "s.goal.unit", 1);
    let action = intercept_goal("/goal", &SessionKind::Claude, &runtime, None)
        .expect("read")
        .expect("handled");
    let GoalAction::Done { .. } = action else {
        panic!("a bare /goal answers locally");
    };
    let events = pulled_events(&conn);
    assert!(
        goal_text_of(&events).is_empty(),
        "a read stores and emits nothing"
    );
    let notices = notices_of(&events);
    assert_eq!(notices.len(), 1);
    assert_eq!(
        notices[0].0, "No goal set.\nUsage: /goal <text>|clear",
        "bare answers the absence plus the usage line"
    );

    runtime.set_goal(Some("Deep work".to_string()));
    let conn = attach_live_agent_for_test(&runtime, "s.goal.unit", 2);
    intercept_goal("/goal", &SessionKind::Claude, &runtime, None)
        .expect("read")
        .expect("handled");
    let events = pulled_events(&conn);
    let notices = notices_of(&events);
    assert_eq!(
        notices
            .iter()
            .map(|notice| notice.0.as_str())
            .collect::<Vec<_>>(),
        ["Goal: Deep work\nUsage: /goal <text>|clear"],
        "bare answers the current goal plus the usage line"
    );
}

#[test]
fn an_overlong_goal_is_refused_with_a_usage_line_and_stores_nothing() {
    let runtime = runtime_without_journal();
    let long = "x".repeat(MAX_GOAL_CHARS + 1);
    let conn = attach_live_agent_for_test(&runtime, "s.goal.unit", 3);
    intercept_goal(
        &format!("/goal {long}"),
        &SessionKind::Claude,
        &runtime,
        None,
    )
    .expect("refusal is an answer, not an error")
    .expect("handled");
    assert_eq!(runtime.goal(), None, "refused text stores nothing");
    let events = pulled_events(&conn);
    assert!(
        goal_text_of(&events).is_empty(),
        "a refusal emits no goal change"
    );
    let notices = notices_of(&events);
    assert_eq!(notices.len(), 1);
    assert!(
        notices[0].0.starts_with(&format!(
            "Goal is too long ({} characters, max {MAX_GOAL_CHARS}).",
            MAX_GOAL_CHARS + 1
        )),
        "unexpected refusal: {}",
        notices[0].0
    );
    assert!(
        notices[0].0.contains("Usage: /goal <text>|clear"),
        "a refusal carries the usage line"
    );
    assert_eq!(notices[0].1, devboule_protocol::NoticeSeverity::Warning);

    // Exactly at the cap still sets.
    let exact = "y".repeat(MAX_GOAL_CHARS);
    let action = intercept_goal(
        &format!("/goal {exact}"),
        &SessionKind::Claude,
        &runtime,
        None,
    )
    .expect("set")
    .expect("handled");
    assert!(
        matches!(action, GoalAction::SendAs(_)),
        "the cap refuses beyond, not at"
    );
}

#[test]
fn a_goal_on_the_next_line_is_a_read_back_not_a_set() {
    // The goal starts on the command's own line: `/goal` alone on its first
    // line is a read-back, and the question below it is never stored as one.
    let runtime = runtime_without_journal();
    let conn = attach_live_agent_for_test(&runtime, "s.goal.unit", 5);
    let action = intercept_goal(
        "/goal\nwhat should I do next?",
        &SessionKind::Claude,
        &runtime,
        None,
    )
    .expect("parse")
    .expect("a bare first line answers locally");
    let GoalAction::Done { .. } = action else {
        panic!("the next line is never the goal");
    };
    assert_eq!(runtime.goal(), None, "nothing is stored from the next line");
    let events = pulled_events(&conn);
    assert!(
        goal_text_of(&events).is_empty(),
        "a bare first line emits no goal change"
    );
    let notices = notices_of(&events);
    assert_eq!(notices.len(), 1);
    assert_eq!(
        notices[0].0, "No goal set.\nUsage: /goal <text>|clear",
        "a bare first line answers the usage line"
    );

    // `/goal fix X` followed by further lines keeps them, as before.
    let runtime = runtime_without_journal();
    let action = intercept_goal(
        "/goal fix X\nand more",
        &SessionKind::Claude,
        &runtime,
        None,
    )
    .expect("parse")
    .expect("a set");
    let GoalAction::SendAs(replacement) = action else {
        panic!("a same-line set with a second line still sets");
    };
    assert_eq!(replacement, "Goal: fix X\nand more");
    assert_eq!(runtime.goal().as_deref(), Some("fix X\nand more"));

    let runtime = runtime_without_journal();
    let action = intercept_goal("  /goal   spaced   ", &SessionKind::Claude, &runtime, None)
        .expect("parse")
        .expect("a set");
    let GoalAction::SendAs(replacement) = action else {
        panic!("surrounding whitespace trims");
    };
    assert_eq!(replacement, "Goal: spaced");
}

#[test]
fn an_uppercase_goal_is_not_a_command() {
    let runtime = runtime_without_journal();
    assert!(
        intercept_goal("/GOAL ship it", &SessionKind::Claude, &runtime, None)
            .expect("parse")
            .is_none(),
        "/GOAL is not the lowercase command"
    );
    assert!(!is_reserved_goal_command("GOAL"));
    assert!(!is_reserved_goal_command("Goal"));
}

/// A stopped session answers but never sends: the message, whatever the
/// transcript's kind and whatever rode beside the text.
fn stopped_message(
    registry: &SessionRegistry,
    id: &str,
    owner: &OwnerId,
    text: &str,
    attachments: &[PromptAttachment],
    references: &[AttachmentReference],
) -> String {
    let conn = ConnHandle::new(44);
    registry
        .send_with_subscription(id, 44, text, attachments, references, owner, &conn)
        .expect_err("a stopped session answers, never sends")
        .message
}

#[test]
fn a_stopped_terminal_transcript_answers_plain_text_with_process_gone() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-gone", "process-goal-gone");
    insert_transcript(&registry, "s.goal.gone", owner.clone());
    // Ordinary text keeps the pre-goal answer, with attachments and without.
    assert_eq!(
        stopped_message(&registry, "s.goal.gone", &owner, "hello there", &[], &[]),
        "This terminal process is gone."
    );
    assert_eq!(
        stopped_message(
            &registry,
            "s.goal.gone",
            &owner,
            "hello there",
            &[png_attachment()],
            &[],
        ),
        "This terminal process is gone."
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_stopped_session_refuses_the_goal() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-gone", "process-goal-gone");
    insert_transcript(&registry, "s.goal.gone", owner.clone());
    assert_eq!(
        stopped_message(&registry, "s.goal.gone", &owner, "/goal ship it", &[], &[]),
        "This session does not accept a goal.",
        "the intercept's own refusal, not the process-gone sentence"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_stopped_agent_transcript_answers_plain_text_with_process_gone() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-gone-agent", "process-goal-gone-agent");
    insert_transcript_with_kind(
        &registry,
        "s.goal.gone.agent",
        owner.clone(),
        SessionKind::Claude,
    );
    // The stored-attachment form answers the same: the kind, not the deck,
    // decides which refusal a stopped session gives.
    assert_eq!(
        stopped_message(
            &registry,
            "s.goal.gone.agent",
            &owner,
            "hello there",
            &[],
            &[stored_reference("s.goal.gone.agent")],
        ),
        "This terminal process is gone."
    );
    assert_eq!(
        stopped_message(
            &registry,
            "s.goal.gone.agent",
            &owner,
            "hello there",
            &[],
            &[],
        ),
        "This terminal process is gone."
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_stopped_agent_transcript_refuses_the_goal() {
    // `insert_transcript` hardcodes a terminal, whose kind arm refuses first;
    // only an agent-kind transcript reaches the transcript guard, so only
    // this test dies when that guard is removed.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-gone-agent", "process-goal-gone-agent");
    insert_transcript_with_kind(
        &registry,
        "s.goal.gone.agent",
        owner.clone(),
        SessionKind::Claude,
    );
    assert_eq!(
        stopped_message(
            &registry,
            "s.goal.gone.agent",
            &owner,
            "/goal ship it",
            &[],
            &[],
        ),
        "This session does not accept a goal."
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_live_terminal_refuses_the_goal_and_types_nothing() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-term", "process-goal-term");
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_terminal_with_writer(
        &registry,
        "s.goal.term",
        owner.clone(),
        None,
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, "s.goal.term", 43);
    let error = registry
        .send_with_subscription("s.goal.term", 43, "/goal ship it", &[], &[], &owner, &conn)
        .expect_err("a terminal refuses the goal");
    assert_eq!(error.message, "This session does not accept a goal.");
    assert!(
        received.lock().expect("writer").is_empty(),
        "the refusal types nothing into the PTY"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_goal_with_attachments_is_refused_and_reaches_no_provider() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-attach", "process-goal-attach");
    let received = Arc::new(Mutex::new(Vec::new()));
    let (runtime, conn) = live_agent(
        &registry,
        "s.goal.attach",
        &owner,
        SessionKind::Claude,
        &received,
    );
    let error = registry
        .send_with_subscription(
            "s.goal.attach",
            41,
            "/goal ship it",
            &[png_attachment()],
            &[],
            &owner,
            &conn,
        )
        .expect_err("a /goal carrying attachments is refused");
    assert_eq!(
        error.message, "Send /goal without attachments.",
        "one plain line, never the provider's words"
    );
    assert_eq!(runtime.goal(), None, "the refused text stores nothing");
    assert!(
        received.lock().expect("writer").is_empty(),
        "nothing reaches the provider"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_goal_with_attachments_is_refused_on_the_native_road_too() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-attach-native", "process-goal-attach-native");
    let received = Arc::new(Mutex::new(Vec::new()));
    registry
        .journal
        .as_ref()
        .expect("the test registry has a journal")
        .create_session(crate::journal::new_session_record(
            "s.goal.attach.native",
            owner.user.clone(),
            None,
            SessionKind::Codex,
            "Agent",
        ))
        .expect("birth row");
    let runtime = insert_live_agent_with_out_of_band(
        &registry,
        "s.goal.attach.native",
        owner.clone(),
        SessionKind::Codex,
        Box::new(RecordingWriter(Arc::clone(&received))),
        Some(Arc::new(ClaimingOutOfBand)),
    );
    let conn = attach_live_agent_for_test(&runtime, "s.goal.attach.native", 46);
    let error = registry
        .send_with_subscription(
            "s.goal.attach.native",
            46,
            "/goal ship it",
            &[png_attachment()],
            &[],
            &owner,
            &conn,
        )
        .expect_err("attachments refuse before the native road can claim the text");
    assert_eq!(error.message, "Send /goal without attachments.");
    assert_eq!(runtime.goal(), None);
    assert!(received.lock().expect("writer").is_empty());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_goal_with_a_stored_reference_is_refused_before_any_lookup() {
    // The `attachment_references` half of the refusal: a well-formed
    // reference is refused like an inline deck, before the store is read.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-attach-ref", "process-goal-attach-ref");
    let received = Arc::new(Mutex::new(Vec::new()));
    let (runtime, conn) = live_agent(
        &registry,
        "s.goal.attach.ref",
        &owner,
        SessionKind::Claude,
        &received,
    );
    let error = registry
        .send_with_subscription(
            "s.goal.attach.ref",
            41,
            "/goal ship it",
            &[],
            &[stored_reference("s.goal.attach.ref")],
            &owner,
            &conn,
        )
        .expect_err("a /goal naming a stored attachment is refused");
    assert_eq!(error.message, "Send /goal without attachments.");
    assert_eq!(runtime.goal(), None, "the refused text stores nothing");
    assert!(
        received.lock().expect("writer").is_empty(),
        "nothing reaches the provider"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
