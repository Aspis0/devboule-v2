//! The live plan-usage cache a fresh attach is served.

use super::super::*;
use super::*;

use serde_json::json;

use super::test_support::{attach_live_agent_replay, attach_tracked, drain};

/// Serialises the tests below: the latest-live-plan-usage cache is
/// daemon-global, so tests that seed it must not overlap.
static PLAN_USAGE_CACHE_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A live agent of `kind` over more than one journalled row, so a rate-limit
/// envelope can sit beside a neighbour.
fn live_agent_replay_fixture_with(
    session_id: &str,
    kind: SessionKind,
    records: Vec<crate::journal::EventRecord>,
) -> (
    std::path::PathBuf,
    Arc<Journal>,
    Arc<SessionRuntime>,
    Arc<ConnHandle>,
) {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-agent-replay-plan-usage");
    let session = new_session_record(session_id, "S-1-5-21-1", None, kind.clone(), "Agent");
    let (journal, runtime, conn) = attach_live_agent_replay(&dir, session, Some(kind), records);
    (dir, journal, runtime, conn)
}

fn rate_limit_record(session_id: &str, seq: u64) -> crate::journal::EventRecord {
    crate::journal::acp_envelope_record(
        session_id,
        1,
        seq,
        &json!({
            "type": "rate_limit_event",
            "rate_limit_info": {
                "status": "allowed",
                "unifiedWindows": {"five_hour": {"utilization": 0.33, "resetsAt": 1_790_632_800}}
            },
            "session_id": "00000000-0000-4000-8000-0000000000d1"
        }),
    )
    .expect("rate limit record")
}

#[test]
fn the_pull_delivers_no_plan_usage_from_a_journalled_rate_limit_row() {
    let _guard = PLAN_USAGE_CACHE_TESTS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    crate::plan_usage_cache::reset();
    let session_id = "s.pull.plan-usage.replay-only";
    let neighbour = crate::journal::acp_envelope_record(
        session_id,
        1,
        2,
        &json!({
            "type": "assistant",
            "message": {"id": "m1", "role": "assistant",
                "content": [{"type": "text", "text": "neighbour"}]}
        }),
    )
    .expect("neighbour record");
    let (dir, journal, runtime, conn) = live_agent_replay_fixture_with(
        session_id,
        SessionKind::Claude,
        vec![rate_limit_record(session_id, 1), neighbour],
    );
    let events = drain(&conn);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::PlanUsage { .. })),
        "replay must not deliver plan usage: {events:?}"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::AgentMessage { text, .. } if text == "neighbour"
    )));

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

fn cached_plan_usage(percent: u64) -> SessionEvent {
    SessionEvent::PlanUsage {
        provider_id: "claude".to_string(),
        plan_label: None,
        windows: vec![devboule_protocol::PlanWindow {
            duration_mins: 300,
            used_percent: Some(percent),
            resets_at: Some(1_790_700_000),
        }],
        credits: None,
    }
}

#[test]
fn a_fresh_attach_delivers_the_cached_live_plan_usage_once() {
    let _guard = PLAN_USAGE_CACHE_TESTS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    crate::plan_usage_cache::reset();
    let session_id = "s.pull.plan-usage.first-attach";
    crate::plan_usage_cache::note_live(&cached_plan_usage(41));
    let (dir, journal, runtime, conn) = live_agent_replay_fixture_with(
        session_id,
        SessionKind::Claude,
        vec![rate_limit_record(session_id, 1)],
    );
    let delivered: Vec<SessionEvent> = drain(&conn)
        .into_iter()
        .filter(|event| matches!(event, SessionEvent::PlanUsage { .. }))
        .collect();
    assert_eq!(
        delivered.len(),
        1,
        "the attach delivers the cached live frame once: {delivered:?}"
    );
    assert_eq!(delivered[0], cached_plan_usage(41));

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_second_attach_delivers_the_cache_again_and_a_live_frame_updates_it() {
    let _guard = PLAN_USAGE_CACHE_TESTS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    crate::plan_usage_cache::reset();
    let session_id = "s.pull.plan-usage.reattach";
    crate::plan_usage_cache::note_live(&cached_plan_usage(41));
    let (dir, journal, runtime, conn) = live_agent_replay_fixture_with(
        session_id,
        SessionKind::Claude,
        vec![rate_limit_record(session_id, 1)],
    );
    let first: Vec<SessionEvent> = drain(&conn)
        .into_iter()
        .filter(|event| matches!(event, SessionEvent::PlanUsage { .. }))
        .collect();
    assert_eq!(first.len(), 1, "the first attach delivers the cache");

    // A second viewer attaches: the cache is a reading, not a handoff, so
    // it is delivered again.
    attach_tracked(&runtime, &conn);
    let second: Vec<SessionEvent> = drain(&conn)
        .into_iter()
        .filter(|event| matches!(event, SessionEvent::PlanUsage { .. }))
        .collect();
    assert_eq!(second.len(), 1, "the second attach delivers it again");
    assert_eq!(second[0], cached_plan_usage(41));

    // A live frame updates the cache; the next attach sees the update.
    crate::plan_usage_cache::note_live(&cached_plan_usage(57));
    attach_tracked(&runtime, &conn);
    let third: Vec<SessionEvent> = drain(&conn)
        .into_iter()
        .filter(|event| matches!(event, SessionEvent::PlanUsage { .. }))
        .collect();
    assert_eq!(third.len(), 1, "the updated cache is delivered once");
    assert_eq!(third[0], cached_plan_usage(57));

    crate::plan_usage_cache::reset();
    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}
