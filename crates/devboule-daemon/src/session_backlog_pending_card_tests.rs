//! A flood of agent frames must not evict a still-pending permission card from
//! the shared backlog: the card is the only road onto a client that attaches
//! after the request arrived, and the agent is waiting on it.
//!
//! Every frame here goes through a publish with no observer attached, so the
//! backlog is the only queue that holds it and the 64-frame / 256 KiB bound is
//! the only thing under test. The attach rules are
//! `session_attach_resume_permission_tests.rs`'s.

use devboule_protocol::{Cursor, SessionEvent, SessionResumeOutcome};

use super::session_attach_resume_fixtures::{answer, texts, AttachFixture};
use super::{PENDING_OUTPUT_BUDGET_BYTES, PENDING_OUTPUT_BUDGET_FRAMES};

/// A session whose journal ends at seq 3 and whose live head is `next_seq`: the
/// cards below sit above the cursor [`attach_from_the_journal_end`] resumes from.
fn fixture(tag: &str, next_seq: u64) -> AttachFixture {
    let f = AttachFixture::new(tag);
    for seq in 2..=3 {
        f.journal
            .append_blocking(
                crate::journal::agent_report_record(
                    f.id.clone(),
                    1,
                    seq,
                    &answer(&format!("journalled-{seq}")),
                )
                .expect("an agent message is reportable"),
            )
            .expect("the row lands");
    }
    f.journal.flush().expect("flush");
    f.runtime.test_live_agent_at(1, next_seq);
    f
}

fn park_card(f: &AttachFixture, tool_call_id: &str, seq: u64) {
    f.runtime.publish_agent_event_with_seq(
        super::permission_broker::permission(tool_call_id),
        None,
        Some(seq),
    );
}

fn flood_text(label: &str, index: u64, padding: usize) -> String {
    format!("{label}-{index:04}-{}", "x".repeat(padding))
}

/// `frames` daemon-local messages of `padding` bytes each, named by `label`.
fn flood(f: &AttachFixture, label: &str, frames: u64, padding: usize) {
    for index in 0..frames {
        f.runtime.publish_agent_event_with_seq(
            answer(&flood_text(label, index, padding)),
            None,
            None,
        );
    }
}

fn backlog_extent(f: &AttachFixture) -> (usize, u64) {
    let stream = f.runtime.stream.lock().expect("stream lock");
    (stream.agent_backlog_bytes, stream.agent_backlog_frames)
}

/// What a client resuming from the journal's end is handed, in order.
fn attach_from_the_journal_end(f: &AttachFixture, conn_id: u64) -> Vec<SessionEvent> {
    let conn = f.conn(conn_id, true);
    let resume = f
        .registry
        .attach_with_subscription(
            &f.id,
            1,
            Some(Cursor {
                generation: 1,
                seq: 3,
            }),
            &conn,
            &f.owner,
            true,
        )
        .expect("attach")
        .expect("a negotiated connection gets the outcome");
    assert_eq!(resume.resume, SessionResumeOutcome::Resumed);
    f.drain(&conn)
}

fn cards(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::PermissionRequest { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_frame_count_flood_leaves_a_pending_card_in_the_backlog() {
    let f = fixture("backlog-card-frames", 5);
    park_card(&f, "card-before-the-flood", 4);
    flood(&f, "frame", PENDING_OUTPUT_BUDGET_FRAMES * 2, 8);

    assert_eq!(
        cards(&attach_from_the_journal_end(&f, 31)),
        vec!["card-before-the-flood"],
        "the agent is still waiting on this card; a bound that dropped it \
         leaves nobody able to answer"
    );

    f.shutdown();
}

#[test]
fn a_byte_flood_leaves_a_pending_card_in_the_backlog() {
    let f = fixture("backlog-card-bytes", 5);
    park_card(&f, "card-before-the-bytes", 4);
    // Frames of 100 KiB: far under the frame bound, enough to pass the byte bound.
    let padding = 100 * 1024;
    flood(
        &f,
        "bulk",
        (PENDING_OUTPUT_BUDGET_BYTES / padding + 2) as u64,
        padding,
    );

    assert_eq!(
        cards(&attach_from_the_journal_end(&f, 32)),
        vec!["card-before-the-bytes"]
    );

    f.shutdown();
}

#[test]
fn a_flood_with_no_pending_card_keeps_only_what_the_bound_always_kept() {
    let f = fixture("backlog-no-card", 4);
    let frames = PENDING_OUTPUT_BUDGET_FRAMES * 2;
    let padding = 8;
    flood(&f, "frame", frames, padding);

    // The frame that crosses the bound clears the queue down to itself, and
    // the frames after it accumulate again.
    let held = frames - PENDING_OUTPUT_BUDGET_FRAMES;
    let first_held = PENDING_OUTPUT_BUDGET_FRAMES;
    let per_frame = serde_json::to_vec(&answer(&flood_text("frame", 0, padding)))
        .expect("serialises")
        .len();
    assert_eq!(
        backlog_extent(&f),
        (per_frame * held as usize, held),
        "the frames and bytes left"
    );

    let events = attach_from_the_journal_end(&f, 33);
    let expected: Vec<String> = (first_held..frames)
        .map(|index| flood_text("frame", index, padding))
        .collect();
    assert_eq!(texts(&events), expected);
    assert!(cards(&events).is_empty());

    f.shutdown();
}

#[test]
fn two_pending_cards_survive_a_flood_in_the_order_they_were_parked() {
    let f = fixture("backlog-two-cards", 6);
    park_card(&f, "card-first", 4);
    flood(&f, "between", PENDING_OUTPUT_BUDGET_FRAMES + 8, 8);
    park_card(&f, "card-second", 5);
    flood(&f, "after", PENDING_OUTPUT_BUDGET_FRAMES + 8, 8);

    assert_eq!(
        cards(&attach_from_the_journal_end(&f, 34)),
        vec!["card-first", "card-second"]
    );

    f.shutdown();
}

#[test]
fn a_card_answered_during_the_flood_is_not_handed_over_again() {
    let f = fixture("backlog-resolved-card", 5);
    park_card(&f, "card-answered-midway", 4);
    flood(&f, "before", PENDING_OUTPUT_BUDGET_FRAMES + 8, 8);
    f.runtime.remove_permission_request("card-answered-midway");
    flood(&f, "after", PENDING_OUTPUT_BUDGET_FRAMES + 8, 8);

    let events = attach_from_the_journal_end(&f, 35);
    assert!(
        cards(&events).is_empty(),
        "an answered card must not come back: {:?}",
        cards(&events)
    );

    f.shutdown();
}
