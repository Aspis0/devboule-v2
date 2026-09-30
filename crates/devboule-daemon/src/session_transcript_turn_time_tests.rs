//! The turn rail's time on a recovered transcript, driven through the real
//! hydrate-and-pull road, because the claim is about what a client is served
//! after a daemon restart — not about a helper. The row shapes the journal
//! holds: a composer row carrying its own `at_ms` (the protocol-17 writer),
//! a composer row from between `message_kind` and `at_ms` (timed from the
//! row's `ts_ms`, the same fill the live replay seam does), an a2a relay (no
//! turn time), a kind-less native row older than `message_kind` (it decodes
//! as `Unknown`, but the daemon itself wrote it, so its `ts_ms` times the
//! turn), and a provider envelope's `user_message_chunk` echo — the live
//! client drops that echo before journaling, so it has no live time and
//! replay gives it none.

use std::sync::Arc;

use rusqlite::Connection;

use super::tests::{ended_record, test_owner, tmp_delete_registry};
use super::{compose_session_id, ConnHandle, SessionEvent, SessionRegistry};
use crate::journal::{output_record, snapshot_limits, Journal};
use crate::paths::RuntimePaths;
use devboule_protocol::{UserMessageAuthor, UserMessageKind};

const T_WITH_FIELD: u64 = 1_759_000_000_000;
const T_BEFORE_FIELD: u64 = 1_759_000_060_000;
const T_A2A: u64 = 1_759_000_120_000;
const T_LEGACY: u64 = 1_759_000_180_000;

fn composer(text: &str, kind: UserMessageKind, at_ms: Option<u64>) -> SessionEvent {
    SessionEvent::AgentUserMessage {
        message_id: None,
        text: text.to_string(),
        author: UserMessageAuthor::Human,
        message_kind: kind,
        at_ms,
    }
}

fn record_row(
    journal: &crate::journal::Journal,
    id: &str,
    seq: u64,
    ts_ms: u64,
    event: &SessionEvent,
) {
    let record = crate::journal::agent_report_record_at(id.to_string(), 1, seq, event, ts_ms)
        .expect("the event is reportable");
    journal.append_blocking(record).expect("the row lands");
}

/// The times the pull serves, per user turn, in wire order.
fn served_turn_times(conn: &ConnHandle) -> Vec<(String, Option<u64>)> {
    conn.pull_events()
        .into_iter()
        .filter_map(|pending| match pending.envelope.event {
            SessionEvent::AgentUserMessage { text, at_ms, .. } => Some((text, at_ms)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_recovered_transcript_serves_each_turn_its_own_row_time() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-turn-time", "process-1717");
    let id = compose_session_id(&owner.session_token(), "turntime").expect("id");
    journal
        .upsert_blocking(ended_record(&id, &owner.user))
        .expect("session row");
    record_row(
        &journal,
        &id,
        1,
        T_WITH_FIELD + 7,
        &composer(
            "protocol-17 writer",
            UserMessageKind::Composer,
            Some(T_WITH_FIELD),
        ),
    );
    record_row(
        &journal,
        &id,
        2,
        T_BEFORE_FIELD,
        &composer("before the field", UserMessageKind::Composer, None),
    );
    record_row(
        &journal,
        &id,
        3,
        T_A2A,
        &composer("an a2a relay", UserMessageKind::OutgoingA2a, None),
    );
    // The payload as the pre-`message_kind` writer stored it: no
    // `messageKind`, no `atMs`.
    let legacy = serde_json::json!({
        "type": "agent_user_message",
        "messageId": "m4",
        "text": "older than message_kind",
        "author": "human"
    });
    journal
        .append_blocking(crate::journal::EventRecord {
            session_id: id.clone(),
            generation: 1,
            seq: 4,
            kind: crate::journal::EventKind::AgentReport,
            ts_ms: T_LEGACY,
            payload: serde_json::to_vec(&legacy).unwrap(),
        })
        .expect("the row lands");
    // A historical ACP envelope can decode as `Unknown`; the live ACP
    // client suppresses the redundant echo before journaling or publishing
    // it, so the row carries no live time to preserve.
    let echo = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": "acp-1", "update": {
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "an acp echo of the prompt"}
        }}
    });
    journal
        .append_blocking(
            crate::journal::acp_envelope_record(&id, 1, 5, &echo).expect("the envelope serializes"),
        )
        .expect("the row lands");

    let conn = ConnHandle::new(1);
    registry
        .attach(&id, None, &conn, &owner, false)
        .expect("the hydration runs");

    assert_eq!(
        served_turn_times(&conn),
        vec![
            ("protocol-17 writer".to_string(), Some(T_WITH_FIELD)),
            ("before the field".to_string(), Some(T_BEFORE_FIELD)),
            ("an a2a relay".to_string(), None),
            ("older than message_kind".to_string(), Some(T_LEGACY)),
            ("an acp echo of the prompt".to_string(), None),
        ],
        "the pull times each composer turn by the live replay's rule"
    );
}

/// The long-session road: once an output snapshot covers a generation's
/// head, agent_report rows at or below its `up_to` replay through the
/// covered-report branch, not the ordinary row scan — the one branch the
/// fixture above never enters. The fixture proves coverage in place (the
/// snapshot's `up_to` passes the composer row's seq) and the pull still
/// serves the composer turn with its row's time.
/// Mutant: the covered `agent_report` push carrying `None` — this fixture
/// is what exercises it.
#[test]
fn a_composer_row_under_a_snapshot_keeps_its_row_time() {
    const T_COVERED: u64 = 1_759_000_240_000;
    let dir = crate::test_dirs::test_temp_dir("devboule-turn-time-covered");
    let journal = Arc::new(
        Journal::open_with_limits(&dir.join("journal.db"), snapshot_limits()).expect("journal"),
    );
    let registry = SessionRegistry::new(RuntimePaths::from_dir(&dir), Some(Arc::clone(&journal)));
    let owner = test_owner("S-1-5-21-turn-time-2", "process-2727");
    let id = compose_session_id(&owner.session_token(), "turntime2").expect("id");
    journal
        .upsert_blocking(ended_record(&id, &owner.user))
        .expect("session row");
    journal
        .append_blocking(output_record(id.clone(), 1, 1, b"chunk-01........"))
        .expect("output");
    // The composer row sits at seq 2; the output bytes below push the
    // session over `snapshot_every_bytes`, so the snapshot's `up_to` covers
    // it while the row itself is never compacted.
    record_row(
        &journal,
        &id,
        2,
        T_COVERED,
        &composer("under a snapshot", UserMessageKind::Composer, None),
    );
    for seq in 3..=6 {
        journal
            .append_blocking(output_record(
                id.clone(),
                1,
                seq,
                format!("chunk-{seq:02}........").as_bytes(),
            ))
            .expect("output");
    }

    journal.flush().expect("flush");
    let db = Connection::open(dir.join("journal.db")).expect("open journal");
    let (snapshots, up_to): (i64, u64) = db
        .query_row(
            "SELECT COUNT(*), COALESCE(MAX(up_to_seq), 0) FROM snapshots
             WHERE session_id = ?1 AND generation = 1",
            [&id],
            |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)),
        )
        .expect("the snapshot row");
    assert!(
        snapshots >= 1 && up_to > 2,
        "the fixture must snapshot over the composer row's seq: \
         {snapshots} snapshots, up_to {up_to}"
    );

    let conn = ConnHandle::new(1);
    registry
        .attach(&id, None, &conn, &owner, false)
        .expect("the hydration runs");

    assert_eq!(
        served_turn_times(&conn),
        vec![("under a snapshot".to_string(), Some(T_COVERED))],
        "a covered composer turn keeps its row's time through the covered-report branch"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
