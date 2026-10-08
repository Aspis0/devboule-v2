//! The live meter's delivery, on a journalled Pi runtime: a reading reaches an
//! attached client without a re-attach, a reading never reaches a replay, a
//! model switch starts and stops the meter, and a client that did not agree the
//! name is never sent one.

use std::sync::Arc;

use devboule_protocol::{PlanWindow, SessionEvent, SessionKind};

use super::{publish_to, watch};
use crate::journal::{new_session_record, Journal};
use crate::session::{ConnHandle, SessionRuntime};

fn reading(observed_at_ms: i64) -> SessionEvent {
    SessionEvent::PlanUsage {
        provider_id: "opencode-go".to_string(),
        plan_label: Some("OpenCode Go".to_string()),
        windows: vec![PlanWindow {
            duration_mins: 300,
            used_percent: Some(7),
            resets_at: None,
        }],
        credits: None,
        observed_at_ms: Some(observed_at_ms),
    }
}

fn manifest(model_provider: &str) -> SessionEvent {
    SessionEvent::SessionManifest {
        provider_id: Some("pi".to_string()),
        current_model_id: Some("model".to_string()),
        current_model_provider_id: Some(model_provider.to_string()),
        models: Vec::new(),
        modes: None,
    }
}

/// Pull until the connection's queue is empty, recording delivery like the
/// connection writer does.
fn drain(conn: &ConnHandle) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    loop {
        let batch = conn.pull_events();
        if batch.is_empty() {
            return events;
        }
        for event in &batch {
            conn.event_sent(event);
        }
        events.extend(batch.into_iter().map(|pending| pending.envelope.event));
    }
}

fn has_reading(events: &[SessionEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, SessionEvent::PlanUsage { .. }))
}

/// A Pi runtime on a fresh journal whose stored manifest names `model_provider`,
/// attached by one connection that did or did not agree `session.plan_usage`.
fn attached_pi(
    name: &str,
    model_provider: &str,
    negotiated: bool,
) -> (Arc<Journal>, Arc<SessionRuntime>, Arc<ConnHandle>) {
    let dir = crate::test_dirs::test_temp_dir(name);
    let session_id = format!("s.quota-live.{name}");
    let session = new_session_record(&session_id, "S-1-5-21-1", None, SessionKind::Pi, "Agent");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    journal.upsert_blocking(session).unwrap();
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.clone(),
        Some(Arc::clone(&journal)),
    ));
    runtime.set_agent_kind(SessionKind::Pi);
    runtime.store_session_manifest(manifest(model_provider));
    let conn = ConnHandle::new(1);
    conn.set_plan_usage_live_negotiated(negotiated);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        &session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    (journal, runtime, conn)
}

#[test]
fn a_reading_published_while_attached_reaches_the_client_without_a_reattach() {
    let (_journal, runtime, conn) = attached_pi("quota-live-attached", "opencode", true);
    watch(&runtime);
    drain(&conn);
    publish_to(&[Arc::clone(&runtime)], &reading(1_000));
    let events = drain(&conn);
    assert!(events.contains(&reading(1_000)), "{events:?}");
}

#[test]
fn a_live_reading_is_not_journaled_so_no_replay_holds_it() {
    let (_journal, runtime, conn) = attached_pi("quota-live-replay", "opencode", true);
    watch(&runtime);
    publish_to(&[Arc::clone(&runtime)], &reading(2_000));
    assert!(
        has_reading(&drain(&conn)),
        "the attached client gets the reading"
    );
    // A second client attaches from the start, as a replay would.
    let replay = ConnHandle::new(2);
    replay.set_plan_usage_live_negotiated(true);
    let outcome = runtime
        .try_attach_with_replay(None, &replay, true)
        .expect("replay attach");
    replay.track_with_agent_replay(
        "s.quota-live.quota-live-replay",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let replayed = drain(&replay);
    assert!(
        !has_reading(&replayed),
        "replay held a reading: {replayed:?}"
    );
}

#[test]
fn a_model_switch_starts_and_stops_the_meter() {
    let (_journal, runtime, conn) = attached_pi("quota-live-switch", "anthropic", true);
    watch(&runtime);
    drain(&conn);
    publish_to(&[Arc::clone(&runtime)], &reading(3_000));
    assert!(
        !has_reading(&drain(&conn)),
        "off an OpenCode model the meter is silent"
    );
    // The same two steps `set_model` takes: store the new manifest, publish it.
    let switched = runtime.store_session_manifest(manifest("opencode"));
    let _ = runtime.publish_agent_event(switched, None);
    drain(&conn);
    publish_to(&[Arc::clone(&runtime)], &reading(4_000));
    assert!(
        has_reading(&drain(&conn)),
        "switched to an OpenCode model: the meter starts"
    );
    let back = runtime.store_session_manifest(manifest("anthropic"));
    let _ = runtime.publish_agent_event(back, None);
    drain(&conn);
    publish_to(&[Arc::clone(&runtime)], &reading(5_000));
    assert!(
        !has_reading(&drain(&conn)),
        "switched away from OpenCode: the meter stops"
    );
}

#[test]
fn a_client_that_did_not_agree_the_name_is_never_sent_a_live_reading() {
    let (_journal, runtime, conn) = attached_pi("quota-live-unagreed", "opencode", false);
    watch(&runtime);
    drain(&conn);
    publish_to(&[Arc::clone(&runtime)], &reading(6_000));
    assert!(!has_reading(&drain(&conn)));
}
