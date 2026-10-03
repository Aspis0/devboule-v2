//! What a reset hands a client and what its observer receives next: the tail in
//! the reply, then the run that follows it.
//!
//! The behaviour under test is the seam, not the decision — the outcome's own
//! order is in `journal_resume_tests.rs`. What must hold here is that a reset
//! registers the observer on the current generation and that the tail, the
//! watermark and the first live event are one ordered run with nothing twice
//! and nothing missing, including when the journal writer is still behind the
//! head the attach captured.

use devboule_protocol::{Cursor, SessionResumeOutcome, SessionResumeReason};

use super::session_attach_resume_fixtures::{answer, texts, AttachFixture};

fn fixture() -> AttachFixture {
    AttachFixture::new("process-attach-resume")
}

#[test]
fn a_stale_cursor_attaches_as_an_epoch_change_and_the_live_run_follows_the_tail() {
    let f = fixture();
    f.runtime.test_publish_journaled(answer("history one"));
    f.runtime.test_publish_journaled(answer("history two"));
    f.journal.flush().expect("flush");
    let conn = f.conn(6, true);

    let resume = f
        .registry
        .attach_with_subscription(
            &f.id,
            1,
            Some(Cursor {
                generation: 0,
                seq: 2,
            }),
            &conn,
            &f.owner,
            false,
        )
        .expect("attach")
        .expect("a negotiated connection gets the outcome");

    let SessionResumeOutcome::Reset { reason, tail } = &resume.resume else {
        panic!("a cursor from another generation must reset");
    };
    assert_eq!(*reason, SessionResumeReason::EpochChanged);
    assert_eq!(
        texts(&tail.events),
        vec!["history one", "history two"],
        "the tail is the current generation's newest events"
    );
    assert!(tail.cursor.generation == 1);
    assert_eq!(tail.cursor.seq, 2);

    // The first live event after the reset must land on top of the tail, once,
    // without the client attaching again.
    f.runtime.test_publish_journaled(answer("live one"));
    f.runtime.test_publish_journaled(answer("live two"));
    let delivered = texts(&f.drain(&conn));

    let mut whole = texts(&tail.events);
    whole.extend(delivered.clone());
    assert_eq!(
        delivered
            .iter()
            .filter(|text| *text == "history two")
            .count(),
        0,
        "the tail's events must not be delivered a second time: {delivered:?}"
    );
    assert_eq!(
        delivered,
        vec!["live one", "live two"],
        "live events follow the reset in order"
    );
    let mut replayed = texts(&tail.events);
    replayed.extend(delivered);
    assert_eq!(
        replayed,
        vec!["history one", "history two", "live one", "live two"],
        "tail then live is one ordered run with nothing twice"
    );

    f.shutdown();
}

#[test]
fn a_publish_just_before_a_reset_attach_is_delivered_once() {
    let f = fixture();
    // Two rows journalled and sitting in the shared backlog when the attach
    // reads the tail: the overlap window the replay/live seam exists for.
    f.runtime.test_publish_journaled(answer("queued one"));
    f.runtime.test_publish_journaled(answer("queued two"));
    f.journal.flush().expect("flush");
    let conn = f.conn(7, true);

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
            false,
        )
        .expect("attach")
        .expect("outcome");
    let SessionResumeOutcome::Reset { tail, .. } = &resume.resume else {
        panic!("expected a reset");
    };
    assert_eq!(texts(&tail.events), vec!["queued one", "queued two"]);

    let delivered = texts(&f.drain(&conn));
    for text in ["queued one", "queued two"] {
        assert_eq!(
            delivered.iter().filter(|seen| *seen == text).count(),
            0,
            "{text} crossed the seam twice: {delivered:?}"
        );
    }

    f.shutdown();
}

#[test]
fn a_reset_attach_over_a_journal_that_has_committed_nothing_delivers_the_pending_interval_once() {
    let f = fixture();
    // Two sequences spent and their events pending for the next observer, with
    // no row committed yet: what an attach sees right after a turn starts,
    // because `try_append` is asynchronous.
    f.runtime.test_live_agent_at(1, 3);
    f.runtime
        .publish_agent_event_with_seq(answer("pending one"), None, Some(1));
    f.runtime
        .publish_agent_event_with_seq(answer("pending two"), None, Some(2));
    let conn = f.conn(12, true);

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
            false,
        )
        .expect("attach")
        .expect("outcome");
    let SessionResumeOutcome::Reset { reason, tail } = &resume.resume else {
        panic!("a cursor from another generation must reset");
    };
    assert_eq!(*reason, SessionResumeReason::EpochChanged);
    assert_eq!(
        resume.head, 2,
        "the head is what the stream had spent when the attach captured it"
    );
    assert!(
        tail.events.is_empty(),
        "nothing is committed, so there is no tail to derive"
    );
    assert_eq!(
        tail.cursor,
        Cursor {
            generation: 1,
            seq: 0
        },
        "an empty tail must not claim the head the writer has not reached: \
         the replay-to-live seam delivers the uncommitted interval instead"
    );

    let delivered = texts(&f.drain(&conn));
    assert_eq!(
        delivered,
        vec!["pending one", "pending two"],
        "every event after the tail cursor arrives exactly once"
    );

    f.shutdown();
}
