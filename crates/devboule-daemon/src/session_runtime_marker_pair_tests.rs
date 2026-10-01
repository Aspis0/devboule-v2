//! One topic: the withheld-marker pair must land or not land as one. A
//! journal that refuses the pair's second row must leave neither row
//! behind — a stranded marker suppresses the next genuine `turn_end`'s
//! finish on replay (`pi_view::drive_replay`) — and the refusal must be
//! accounted as the two frames it is.

use std::sync::Arc;

use devboule_protocol::{SessionEvent, SessionKind};

use super::SessionRuntime;
use crate::journal::{acp_envelope_record, new_session_record, Journal};

/// The pair this topic journals: the suppression marker and an envelope
/// that never matters, because the refusal takes both.
fn pair() -> (serde_json::Value, serde_json::Value, u64) {
    let marker = crate::claude_view::withheld_finish_marker();
    let envelope = serde_json::json!({
        "type": "message_update",
        "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "held"}
    });
    let bytes = (serde_json::to_vec(&marker).unwrap().len()
        + serde_json::to_vec(&envelope).unwrap().len()) as u64;
    (marker, envelope, bytes)
}

#[test]
fn a_refused_second_row_leaves_no_marker_to_suppress_the_next_finish() {
    let dir = crate::test_dirs::test_temp_dir("devboule-marker-pair-atomic");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal open"));
    let session_id = "s.marker.pair.atomic";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Pi,
            "Pair",
        ))
        .expect("session row");

    // The journal seam: a genuine `turn_end` already occupies the seq the
    // pair's second row will claim, so the writer's INSERT hits the
    // (session, generation, seq) primary key with row 1 already inside the
    // transaction — the rollback half of the atomicity, where the marker
    // really is written and then taken back. The producer-side refusals
    // (a full queue, a dead writer) are covered by the dead-writer test
    // below. The pair's marker takes seq 1, its envelope seq 2 (a fresh
    // runtime allocates from 1).
    let turn_end = serde_json::json!({
        "type": "turn_end",
        "message": {
            "role": "assistant",
            "content": [],
            "model": "pi-test",
            "usage": {"totalTokens": 42},
            "stopReason": "end_turn"
        },
        "toolResults": []
    });
    journal
        .append_blocking(acp_envelope_record(session_id, 1, 2, &turn_end).expect("record"))
        .expect("planted turn_end");

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    let (marker, envelope, pair_bytes) = pair();
    let seq = runtime
        .journal_agent_envelope_pair(&marker, &envelope)
        .expect("pair attempt reaches the journal");
    assert_eq!(seq, 2, "the pair claims its two adjacent seqs");

    journal.flush().expect("flush");
    // The refusal is observed, not inferred from absence: the writer
    // accounted both rows of the pair it could not commit — two frames
    // and their bytes — under this test's own journal, whose counters
    // started at zero.
    assert_eq!(
        journal.session_drop_counters(session_id),
        (2, pair_bytes),
        "a refused pair is two dropped rows, not one"
    );
    assert_eq!(
        journal.stats().failed_frames,
        2,
        "both rows of the refused pair reach the failed-frame counter"
    );

    let page = journal
        .replay_agent_page(session_id, 1, 1, 0, seq, 10)
        .expect("agent page");
    assert!(
        page.records
            .iter()
            .any(|record| record.seq == 2
                && record.payload == serde_json::to_vec(&turn_end).unwrap()),
        "the seam is real: the planted turn_end row is the refusal: {:?}",
        page.records
    );
    assert!(
        !page.records.iter().any(|record| record.seq == 1),
        "a refused second row must take the marker with it: {:?}",
        page.records
    );

    let replay = journal.replay(session_id).expect("replay");
    assert!(
        replay
            .events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentFinished { .. })),
        "a stranded marker suppresses this turn_end's finish; the refusal must strand nothing: {:?}",
        replay.events
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The producer-side refusal, deterministically: with the writer joined,
/// the channel is disconnected, so `try_append_pair` fails at `try_send`
/// and both rows are refused on the spot — two dropped frames, and the
/// runtime marks the session degraded. No queue to fill, no race.
#[test]
fn a_dead_writer_refuses_both_rows_of_the_pair() {
    let dir = crate::test_dirs::test_temp_dir("devboule-marker-pair-dead-writer");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal open"));
    let session_id = "s.marker.pair.dead";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Pi,
            "Pair",
        ))
        .expect("session row");
    journal.shutdown();

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    let (marker, envelope, pair_bytes) = pair();
    let seq = runtime
        .journal_agent_envelope_pair(&marker, &envelope)
        .expect("the pair attempt still allocates its seqs");
    assert_eq!(seq, 2, "the pair claims its two adjacent seqs");

    assert_eq!(
        journal.session_drop_counters(session_id),
        (2, pair_bytes),
        "a disconnected writer refuses the pair at try_send, one drop per row"
    );
    assert_eq!(
        journal.stats().failed_frames,
        2,
        "both rows of the refused pair reach the failed-frame counter"
    );
    assert!(
        runtime
            .journal_degraded
            .load(std::sync::atomic::Ordering::Acquire),
        "a refused pair marks the session's journal degraded"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
