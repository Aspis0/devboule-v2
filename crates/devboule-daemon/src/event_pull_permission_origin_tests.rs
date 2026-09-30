//! The origin a permission card carries, on publish and on replay.

use super::super::*;
use super::*;

use super::test_support::{drain, live_agent_replay_fixture};

/// §8b A14 at the egress: the placeholder a provider client writes is
/// replaced with the session's stored origin before the card reaches a
/// subscriber, so a peer session's card cannot be shown as this machine's
/// own — and a session whose origin was never installed surfaces as
/// `unknown`, never as `local`: `local` is measured, never assumed.
#[test]
fn a_published_permission_request_carries_the_sessions_stored_origin() {
    let session_id = "s.live.agent.replay.card-origin";
    let record = crate::journal::EventRecord {
        session_id: session_id.to_string(),
        generation: 1,
        seq: 1,
        kind: crate::journal::EventKind::Output,
        ts_ms: 0,
        payload: b"ready".to_vec(),
    };
    let (dir, journal, runtime, conn) = live_agent_replay_fixture(session_id, record);
    // Clear whatever the attach replayed: this asserts what a *publish*
    // hands the subscriber.
    let _ = drain(&conn);

    // Publish the card a provider client builds — placeholder origin and
    // no idea which device asked for the session — and read it back.
    let publish = |runtime: &SessionRuntime, tool_call_id: &str| {
        runtime.publish_agent_event(
            SessionEvent::PermissionRequest {
                tool_call_id: tool_call_id.to_string(),
                title: "Run command".to_string(),
                description: None,
                command: None,
                args: None,
                cwd: None,
                env: None,
                options: Vec::new(),
                is_chooser: None,
                kind: None,
                plan: None,
                questions: None,
                origin: devboule_protocol::SessionOrigin::unknown(),
                create_agent: None,
            },
            None,
        );
    };
    let card_origin = |conn: &ConnHandle| {
        drain(conn)
            .into_iter()
            .find_map(|event| match event {
                SessionEvent::PermissionRequest { origin, .. } => Some(origin),
                _ => None,
            })
            .expect("the subscriber receives the card")
    };

    // Phase one: no stored origin. The placeholder must not survive as
    // `local` — nothing measured this session as this machine's own, so
    // the card says `unknown`. `local` is only ever measured.
    publish(&runtime, "call-unstored");
    assert_eq!(
        card_origin(&conn),
        devboule_protocol::SessionOrigin::unknown(),
        "a session whose origin was never installed reads as unknown, never as local"
    );

    // Phase two: the registry installs the session's stored origin — a
    // paired device's — and the card says that, not the placeholder.
    runtime.set_origin(devboule_protocol::SessionOrigin::peer(
        "device-phone",
        devboule_protocol::PeerRole::Client,
    ));
    publish(&runtime, "call-origin");
    assert_eq!(
        card_origin(&conn),
        devboule_protocol::SessionOrigin::peer("device-phone", devboule_protocol::PeerRole::Client),
        "the placeholder must not survive the egress"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The v9 migration and the replay path together: the bytes the migration
/// writes for a pre-origin permission payload are bytes the live replay
/// parses — no `JournalDegraded`, and the card says `local`.
#[test]
fn a_migrated_permission_payload_replays_as_a_local_card() {
    let session_id = "s.live.agent.replay.migrated-origin";
    // Exactly what a v8 daemon stored for a permission request.
    let legacy = serde_json::to_vec(&serde_json::json!({
        "type": "permission_request",
        "toolCallId": "call-migrated",
        "title": "Run command",
        "options": []
    }))
    .expect("legacy payload");
    let crate::journal::OriginBackfill::Rewritten(payload) =
        crate::journal::payload_with_origin(&legacy)
    else {
        panic!("a pre-origin permission request is what the migration rewrites");
    };
    let record = crate::journal::EventRecord {
        session_id: session_id.to_string(),
        generation: 1,
        seq: 1,
        kind: crate::journal::EventKind::AgentReport,
        ts_ms: 0,
        payload,
    };

    let (dir, journal, runtime, conn) = live_agent_replay_fixture(session_id, record);
    let events = drain(&conn);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::JournalDegraded { .. })),
        "a migrated payload must not degrade the journal: {events:?}"
    );
    let origin = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::PermissionRequest { origin, .. } => Some(origin.clone()),
            _ => None,
        })
        .expect("the replayed card");
    assert_eq!(origin, devboule_protocol::SessionOrigin::local());

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}
