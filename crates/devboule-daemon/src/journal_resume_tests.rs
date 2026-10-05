//! What a reattaching cursor resolves to: the retained floor, a cursor ahead
//! of the head, a cursor under the floor, a hole in the retained rows, and the
//! byte budget the tail is held to.
//!
//! The fixtures stage the journal by hand, because every one of these is a
//! statement about what the journal holds rather than about how the runtime
//! got there — including the hole, which no writer will produce on purpose.

use std::path::PathBuf;
use std::sync::Arc;

use devboule_protocol::{
    Cursor, SessionEvent, SessionKind, SessionResumeOutcome, SessionResumeReason,
};

use crate::journal::{new_session_record, Journal};

use super::{build_tail, resolve, RESET_TAIL_BYTES};
use crate::session::SessionRuntime;

/// A live Claude agent on its own journal, holding `rows` of the given
/// generation as agent reports.
fn live_agent(
    dir: &std::path::Path,
    session_id: &str,
    generation: u64,
    texts: &[&str],
) -> Arc<SessionRuntime> {
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Claude,
            "Agent",
        ))
        .unwrap();
    for (index, text) in texts.iter().enumerate() {
        let seq = index as u64 + 1;
        let event = SessionEvent::AgentMessage {
            message_id: Some(format!("m{seq}")),
            text: (*text).to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,

            images: Vec::new(),
        };
        journal
            .append_blocking(
                crate::journal::agent_report_record(session_id, generation, seq, &event).unwrap(),
            )
            .unwrap();
    }
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    runtime.set_agent_kind(SessionKind::Claude);
    runtime.test_live_agent_at(generation, texts.len() as u64 + 1);
    runtime
}

fn temp(name: &str) -> PathBuf {
    crate::test_dirs::test_temp_dir(name)
}

/// Delete the half-open seq range `[from, until)` of one generation's rows, the
/// shape both retention's prefix trim and a lost append leave behind.
fn trim(dir: &std::path::Path, session_id: &str, from: u64, until: u64) {
    let conn = rusqlite::Connection::open(dir.join("journal.db")).unwrap();
    conn.execute(
        "DELETE FROM events
         WHERE session_id = ?1 AND generation = 1 AND seq >= ?2 AND seq < ?3",
        rusqlite::params![session_id, from as i64, until as i64],
    )
    .unwrap();
}

fn cursor(seq: u64) -> Cursor {
    Cursor { generation: 1, seq }
}

#[test]
fn a_cursor_at_the_floor_boundary_resumes_and_one_below_it_is_compacted() {
    let dir = temp("devboule-resume-floor");
    // Five rows, retention has trimmed the first two, so the floor is 3.
    let runtime = live_agent(&dir, "s.resume.floor", 1, &["a", "b", "c", "d", "e"]);
    // The trim, staged by hand: rows 1 and 2 are gone, which is what
    // `trim_session` leaves behind.
    trim(&dir, "s.resume.floor", 1, 3);

    let boundary = resolve(&runtime, 1, 5, cursor(2)).expect("decision");
    assert_eq!(
        boundary.info.oldest_seq, 3,
        "the floor is the lowest row the generation still holds"
    );
    assert_eq!(boundary.info.head, 5);
    assert_eq!(
        boundary.info.resume,
        SessionResumeOutcome::Resumed,
        "a cursor exactly at oldest_seq - 1 has seen everything retained"
    );
    assert_eq!(boundary.resume_from, 2);

    let below = resolve(&runtime, 1, 5, cursor(1)).expect("decision");
    let SessionResumeOutcome::Reset { reason, tail } = &below.info.resume else {
        panic!(
            "a cursor below the floor must reset, got {:?}",
            below.info.resume
        );
    };
    assert_eq!(*reason, SessionResumeReason::CursorCompacted);
    assert_eq!(tail.cursor.generation, 1);
    assert_eq!(tail.cursor.seq, 5, "the tail ends at the captured head");
    let texts: Vec<String> = tail
        .events
        .iter()
        .map(|event| match event {
            SessionEvent::AgentMessage { text, .. } => text.clone(),
            other => panic!("unexpected tail event {other:?}"),
        })
        .collect();
    assert_eq!(
        texts,
        vec!["c", "d", "e"],
        "the tail is the newest retained events, newest last"
    );
    assert!(
        tail.tail_complete,
        "the tail reached the floor, so nothing before it is missing"
    );
    assert_eq!(
        below.resume_from, 5,
        "the replay after the reply starts at the tail"
    );

    drop(runtime);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cursor_above_the_head_is_ahead_and_gets_the_same_tail() {
    let dir = temp("devboule-resume-ahead");
    let runtime = live_agent(&dir, "s.resume.ahead", 1, &["a", "b", "c"]);
    let ahead = resolve(&runtime, 1, 3, cursor(9)).expect("decision");
    assert_eq!(ahead.info.head, 3);
    assert_eq!(ahead.info.oldest_seq, 1);
    let SessionResumeOutcome::Reset { reason, tail } = &ahead.info.resume else {
        panic!("a cursor past the head must reset");
    };
    assert_eq!(*reason, SessionResumeReason::CursorAhead);
    assert_eq!(tail.cursor.seq, 3);
    assert!(tail.tail_complete, "the whole generation fits the tail");
    assert_eq!(tail.events.len(), 3);

    drop(runtime);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_interior_hole_is_reported_as_a_gap_and_never_as_a_partial_history() {
    let dir = temp("devboule-resume-gap");
    let runtime = live_agent(&dir, "s.resume.gap", 1, &["a", "b", "c", "d"]);
    // The seq 2 row is gone: exactly what a refused `try_append` leaves behind,
    // since the stream had already spent the sequence.
    trim(&dir, "s.resume.gap", 2, 3);

    // A cursor below the hole must not be handed the rows after it as if they
    // were contiguous.
    let below = resolve(&runtime, 1, 4, cursor(0)).expect("decision");
    let SessionResumeOutcome::Reset { reason, .. } = &below.info.resume else {
        panic!(
            "a hole in the replay window must reset, got {:?}",
            below.info.resume
        );
    };
    assert_eq!(*reason, SessionResumeReason::JournalGap);
    assert_eq!(below.info.oldest_seq, 1, "the floor is a row, not the hole");

    // A cursor already past the hole asks for nothing that crosses it, so it
    // resumes: the lost row is behind the reader, not inside its window.
    let past = resolve(&runtime, 1, 4, cursor(3)).expect("decision");
    assert_eq!(past.info.resume, SessionResumeOutcome::Resumed);
    assert_eq!(past.info.oldest_seq, 1);

    drop(runtime);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cursor_from_another_generation_is_an_epoch_change_with_a_current_tail() {
    let dir = temp("devboule-resume-epoch");
    let runtime = live_agent(&dir, "s.resume.epoch", 2, &["one", "two"]);
    let stale = resolve(
        &runtime,
        2,
        2,
        Cursor {
            generation: 1,
            seq: 40,
        },
    )
    .expect("decision");
    let SessionResumeOutcome::Reset { reason, tail } = &stale.info.resume else {
        panic!("another generation must reset");
    };
    assert_eq!(*reason, SessionResumeReason::EpochChanged);
    assert_eq!(
        tail.cursor.generation, 2,
        "the tail belongs to the generation the client is being registered on"
    );
    assert_eq!(tail.cursor.seq, 2);

    drop(runtime);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_tail_walks_backwards_until_the_wire_budget_is_spent() {
    let dir = temp("devboule-resume-budget");
    // Each event carries roughly a kilobyte of wire; a few hundred fit, a few
    // thousand do not.
    let texts: Vec<String> = (0..600)
        .map(|index| format!("{index}{}", "x".repeat(2000)))
        .collect();
    let borrowed: Vec<&str> = texts.iter().map(String::as_str).collect();
    let runtime = live_agent(&dir, "s.resume.budget", 1, &borrowed);
    let domain = runtime.resume_domain(1).expect("domain");
    let tail = build_tail(&runtime, 1, 600, &domain);

    let wire: usize = tail
        .events
        .iter()
        .map(|event| {
            serde_json::to_vec(event)
                .map(|bytes| bytes.len())
                .unwrap_or(0)
        })
        .sum();
    assert!(wire <= RESET_TAIL_BYTES, "tail wire bytes {wire}");
    assert!(!tail.events.is_empty(), "the newest events must fit");
    assert_eq!(tail.cursor.seq, 600, "the tail ends at the captured head");
    assert!(
        !tail.tail_complete,
        "600 rows cannot fit 256 KiB, so the tail stops short of the floor"
    );
    let first = match &tail.events[0] {
        SessionEvent::AgentMessage { text, .. } => text.clone(),
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(first, texts[600 - tail.events.len()]);

    drop(runtime);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_event_over_the_budget_alone_leaves_an_empty_tail_at_the_head() {
    let dir = temp("devboule-resume-oversized");
    let huge = "z".repeat(RESET_TAIL_BYTES * 2);
    let runtime = live_agent(
        &dir,
        "s.resume.huge",
        1,
        &["small", "newest", huge.as_str()],
    );
    let domain = runtime.resume_domain(1).expect("domain");
    let tail = build_tail(&runtime, 1, 3, &domain);
    assert!(
        tail.events.is_empty(),
        "the walk must stop at the oversized event instead of shipping it"
    );
    assert_eq!(
        tail.cursor,
        Cursor {
            generation: 1,
            seq: 3
        },
        "an empty tail resumes from the head"
    );
    assert!(!tail.tail_complete);

    drop(runtime);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_empty_tail_anchors_at_the_last_committed_seq_and_not_at_the_head() {
    let dir = temp("devboule-resume-lag");
    let runtime = live_agent(&dir, "s.resume.lag", 1, &[]);
    // Three sequences spent, nothing committed: the journal writer is behind
    // the head this attach captures.
    runtime.test_live_agent_at(1, 4);
    let decision = resolve(
        &runtime,
        1,
        3,
        Cursor {
            generation: 0,
            seq: 0,
        },
    )
    .expect("decision");
    let SessionResumeOutcome::Reset { reason, tail } = &decision.info.resume else {
        panic!("another generation must reset");
    };
    assert_eq!(*reason, SessionResumeReason::EpochChanged);
    assert_eq!(decision.info.head, 3, "the head is the in-memory watermark");
    assert_eq!(decision.info.oldest_seq, 0, "the generation holds no rows");
    assert!(tail.events.is_empty());
    assert_eq!(
        tail.cursor,
        Cursor {
            generation: 1,
            seq: 0
        },
        "an empty tail names the newest committed seq, which for a generation \
         that holds no rows is its before-first value: the head belongs to the \
         writer, not to the tail"
    );
    assert_eq!(
        decision.resume_from, 0,
        "the replay after the reply covers the uncommitted interval"
    );

    drop(runtime);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_empty_generation_reports_zero_for_both_edges_and_resumes() {
    let dir = temp("devboule-resume-empty");
    let runtime = live_agent(&dir, "s.resume.empty", 1, &[]);
    let resumed = resolve(&runtime, 1, 0, cursor(0)).expect("decision");
    assert_eq!(resumed.info.oldest_seq, 0);
    assert_eq!(resumed.info.head, 0);
    assert_eq!(resumed.info.resume, SessionResumeOutcome::Resumed);

    let ahead = resolve(&runtime, 1, 0, cursor(1)).expect("decision");
    let SessionResumeOutcome::Reset { reason, tail } = &ahead.info.resume else {
        panic!("a cursor above an empty head must reset");
    };
    assert_eq!(*reason, SessionResumeReason::CursorAhead);
    assert!(tail.events.is_empty());
    assert!(tail.tail_complete, "there is nothing before an empty tail");

    drop(runtime);
    let _ = std::fs::remove_dir_all(&dir);
}
