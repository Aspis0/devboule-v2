//! What an attach owes a permission card that is still waiting for an answer:
//! a reset hands it over again, and an ordinary resume leaves what the client's
//! own cursor already covers alone.
//!
//! Today's writers publish a card live and journal no row for it, so the shared
//! backlog is the road onto a client that attaches after the request arrived.
//! The tail a reset hands over is `session_attach_resume_tests.rs`'s.

use devboule_protocol::{Cursor, SessionEvent, SessionResumeOutcome};

use super::session_attach_resume_fixtures::{answer, AttachFixture};

fn fixture() -> AttachFixture {
    AttachFixture::new("attach-pending-permission")
}

/// Publish one card at `seq`, unanswered: the shape a provider's live request
/// leaves when nobody answers it. The stream must already sit at `seq + 1` —
/// the card spends the sequence its envelope spent. The bool this returns says
/// whether a silent session woke, not whether the card was published.
fn park_card(f: &AttachFixture, tool_call_id: &str, seq: u64) {
    f.runtime.publish_agent_event_with_seq(
        super::permission_broker::permission(tool_call_id),
        None,
        Some(seq),
    );
}

/// Commit `rows` journalled agent messages of about two kilobytes each at
/// seqs `2..=rows + 1`, and leave the stream head on the newest of them. They
/// go straight into the journal rather than through a publish: what the test is
/// staging is a conversation the writer committed while a card was waiting, and
/// a publish would put those frames in the shared backlog beside the card and
/// bound it away inside the queue's own budget.
fn flood_above_the_card(f: &AttachFixture, rows: u64) {
    for seq in 2..=rows + 1 {
        let text = format!("{seq}{}", "x".repeat(2000));
        f.journal
            .append_blocking(
                crate::journal::agent_report_record(f.id.clone(), 1, seq, &answer(&text))
                    .expect("an agent message is reportable"),
            )
            .expect("the row lands");
    }
    f.journal.flush().expect("flush");
    f.runtime.test_live_agent_at(1, rows + 2);
}

/// Commit one `agent_report` row holding a card, at `seq`: the shape a daemon
/// before v9 wrote and [`crate::journal::payload_with_origin`] still rewrites
/// in place, so a journal can hold one and the tail's walk derives it.
fn commit_card_row(f: &AttachFixture, tool_call_id: &str, seq: u64) {
    f.journal
        .append_blocking(
            crate::journal::agent_report_record(
                f.id.clone(),
                1,
                seq,
                &super::permission_broker::permission(tool_call_id),
            )
            .expect("a card row serialises"),
        )
        .expect("the row lands");
    f.journal.flush().expect("flush");
}

/// The cards in one delivery, by `tool_call_id`, in order.
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
fn a_reset_attach_hands_over_a_pending_card_the_tail_cannot_carry() {
    let f = fixture();
    // The card is seq 1, and 599 messages follow it: far more than the
    // 256 KiB tail can carry, so the tail cannot reach the card's own seq.
    f.runtime.test_live_agent_at(1, 2);
    park_card(&f, "card-below-the-tail", 1);
    flood_above_the_card(&f, 599);
    let conn = f.conn(21, true);

    let resume = f
        .registry
        .attach_with_subscription(
            &f.id,
            1,
            Some(Cursor {
                generation: 0,
                seq: 0,
            }),
            &conn,
            &f.owner,
            true,
        )
        .expect("attach")
        .expect("a negotiated connection gets the outcome");
    let SessionResumeOutcome::Reset { tail, .. } = &resume.resume else {
        panic!("a cursor from another generation must reset");
    };
    assert!(
        !tail.tail_complete,
        "the tail stops short of the floor, so it carries nothing from seq 1"
    );
    assert!(
        cards(&tail.events).is_empty(),
        "no journal row derives a card, so the tail cannot be the card's road"
    );

    assert_eq!(
        cards(&f.drain(&conn)),
        vec!["card-below-the-tail"],
        "the agent is still waiting on this card: a reset that dropped it \
         leaves nobody able to answer"
    );

    f.shutdown();
}

#[test]
fn a_reset_attach_does_not_hand_over_a_card_its_own_tail_carries() {
    let f = fixture();
    flood_above_the_card(&f, 599);
    // The card's row is the newest one, inside the tail's window, and the same
    // card is still parked in the shared backlog: the reply and the seam could
    // both offer it.
    commit_card_row(&f, "card-inside-the-tail", 601);
    f.runtime.test_live_agent_at(1, 602);
    park_card(&f, "card-inside-the-tail", 601);
    let conn = f.conn(22, true);

    let resume = f
        .registry
        .attach_with_subscription(
            &f.id,
            1,
            Some(Cursor {
                generation: 0,
                seq: 0,
            }),
            &conn,
            &f.owner,
            true,
        )
        .expect("attach")
        .expect("outcome");
    let SessionResumeOutcome::Reset { tail, .. } = &resume.resume else {
        panic!("a cursor from another generation must reset");
    };
    assert_eq!(
        cards(&tail.events),
        vec!["card-inside-the-tail"],
        "the row is inside the window, so the reply carries the card"
    );

    assert_eq!(
        cards(&f.drain(&conn)),
        Vec::<String>::new(),
        "this client already has the card, from the tail in its own reply"
    );

    f.shutdown();
}

#[test]
fn an_ordinary_resume_leaves_a_card_the_client_already_holds_alone() {
    let f = fixture();
    f.runtime.test_live_agent_at(1, 2);
    park_card(&f, "card-below-the-cursor", 1);
    flood_above_the_card(&f, 2);
    let conn = f.conn(23, true);

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
        .expect("outcome");
    assert_eq!(
        resume.resume,
        SessionResumeOutcome::Resumed,
        "a cursor inside the retained range resumes, with no tail to replace \
         the client's timeline"
    );

    assert_eq!(
        cards(&f.drain(&conn)),
        Vec::<String>::new(),
        "the card is at seq 1 and the cursor is at 3, so it is on this client's \
         screen already and a second one would be a duplicate"
    );

    f.shutdown();
}

#[test]
fn an_ordinary_resume_still_hands_over_a_card_the_client_has_not_seen() {
    let f = fixture();
    // The card is seq 4, above the cursor this client resumes from and with no
    // row of its own: a seq the replay has not covered, because a row that
    // derives nothing never enters `replayed_seqs`.
    flood_above_the_card(&f, 2);
    f.runtime.test_live_agent_at(1, 5);
    park_card(&f, "card-above-the-cursor", 4);
    let conn = f.conn(24, true);

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
        .expect("outcome");
    assert_eq!(resume.resume, SessionResumeOutcome::Resumed);

    assert_eq!(
        cards(&f.drain(&conn)),
        vec!["card-above-the-cursor"],
        "a card above the cursor has never reached this client"
    );

    f.shutdown();
}
