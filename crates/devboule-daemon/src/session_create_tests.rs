//! Characterisation tests for the create road: the birth road as it stands,
//! each test naming the mutant it must catch.

use std::sync::Arc;

use devboule_protocol::{ErrorCode, OwnerId, SessionKind, SessionOrigin};

use crate::journal::{PersistStatus, SessionRecord};
use crate::peer_policy::{ConnPeer, PeerRole, TransportBinding};
use crate::profile_delivery::ProfileDelivery;
use crate::server::ServerState;

use super::{PtyCommand, SessionCreateMeta, WireError};

fn road_state(instance: &str, user: &str) -> (Arc<ServerState>, OwnerId) {
    (
        ServerState::new(instance.to_string()),
        OwnerId::new(user, "create-road-client").expect("owner"),
    )
}

fn refused_spawn_command() -> PtyCommand {
    PtyCommand::new(
        "definitely-not-a-real-program-xyz",
        Vec::new(),
        std::env::temp_dir(),
        Vec::new(),
    )
}

fn echo_command() -> PtyCommand {
    PtyCommand::new(
        "cmd.exe",
        vec!["/c".to_string(), "echo create-road".to_string()],
        std::env::temp_dir(),
        Vec::new(),
    )
}

fn remote_peer() -> Option<ConnPeer> {
    Some(ConnPeer::Remote {
        device_id: "dev-phone".to_string(),
        role: PeerRole::Client,
        paired_by_user: None,
        binding: TransportBinding::tailnet("nstable", "node.tailnet.ts.net.", "user@example.com"),
    })
}

#[allow(clippy::too_many_arguments)]
fn create(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    workspace_id: Option<String>,
    kind: SessionKind,
    provider: Option<String>,
    command: Option<PtyCommand>,
    conn_peer: &Option<ConnPeer>,
    meta: &SessionCreateMeta,
) -> Result<devboule_protocol::Session, WireError> {
    state.sessions.create_with_provider_env(
        state,
        owner,
        workspace_id,
        kind,
        provider,
        ProfileDelivery::none(),
        command,
        conn_peer,
        None,
        meta,
    )
}

fn journal_rows(state: &Arc<ServerState>) -> Vec<SessionRecord> {
    state
        .sessions
        .journal
        .as_ref()
        .expect("the test state has a journal")
        .list()
        .expect("journal rows")
}

fn birth_row(state: &Arc<ServerState>, title: &str) -> SessionRecord {
    journal_rows(state)
        .into_iter()
        .find(|row| row.title == title)
        .expect("the birth row")
}

/// Mutant: the `meta.cwd` override hoisted above the workspace lookup —
/// an unknown workspace must still refuse, not start in the carried cwd.
/// Code and message alone cannot discriminate: the family's spawn-error
/// mapping answers the same once a row exists, so the refusal must also
/// leave the journal empty.
#[test]
fn an_unknown_workspace_refuses_even_when_the_create_carries_a_cwd() {
    let (state, owner) = road_state("create-order-workspace", "S-1-5-21-create-ws");
    let meta = SessionCreateMeta {
        cwd: Some(std::env::temp_dir()),
        ..SessionCreateMeta::default()
    };
    let error = create(
        &state,
        &owner,
        Some("w.missing".to_string()),
        SessionKind::Terminal,
        None,
        Some(refused_spawn_command()),
        &None,
        &meta,
    )
    .expect_err("the unknown workspace must refuse the create");
    assert_eq!(error.code, ErrorCode::WorkspaceUnavailable);
    assert!(
        error.message.contains("w.missing"),
        "refusal names ws: {}",
        error.message
    );
    let rows = journal_rows(&state);
    assert!(
        rows.is_empty(),
        "the refusal fired before the birth door; no row may exist, found {}",
        rows.len()
    );
}

/// Mutant: `meta.origin` demoted below the connection-derived origin —
/// a carried (creator's) origin must win over the road the ask arrived on.
#[test]
fn a_carried_origin_beats_the_connection_it_arrives_on() {
    let (state, owner) = road_state("create-origin-carried", "S-1-5-21-create-origin");
    let meta = SessionCreateMeta {
        origin: Some(SessionOrigin::local()),
        ..SessionCreateMeta::default()
    };
    create(
        &state,
        &owner,
        None,
        SessionKind::Terminal,
        None,
        Some(refused_spawn_command()),
        &remote_peer(),
        &meta,
    )
    .expect_err("the fake binary refuses the spawn");
    let row = birth_row(&state, "Terminal");
    assert_eq!(row.origin, SessionOrigin::local());
}

/// Mutant: the connection-derived origin dropped or hard-coded local —
/// a remote connection's birth row names the paired device and its role.
#[test]
fn a_remote_connection_birth_stamps_the_peer_origin() {
    let (state, owner) = road_state("create-origin-remote", "S-1-5-21-create-remote");
    create(
        &state,
        &owner,
        None,
        SessionKind::Terminal,
        None,
        Some(refused_spawn_command()),
        &remote_peer(),
        &SessionCreateMeta::default(),
    )
    .expect_err("the fake binary refuses the spawn");
    let row = birth_row(&state, "Terminal");
    assert_eq!(
        row.origin,
        SessionOrigin::peer("dev-phone", PeerRole::Client)
    );
}

/// Mutant: the no-connection default origin — a plain local create stamps
/// `local`, and an unnamed terminal is titled by its kind.
#[test]
fn a_local_terminal_birth_stamps_local_origin_and_the_kind_title() {
    let (state, owner) = road_state("create-origin-local", "S-1-5-21-create-local");
    create(
        &state,
        &owner,
        None,
        SessionKind::Terminal,
        None,
        Some(refused_spawn_command()),
        &None,
        &SessionCreateMeta::default(),
    )
    .expect_err("the fake binary refuses the spawn");
    let row = birth_row(&state, "Terminal");
    assert_eq!(row.origin, SessionOrigin::local());
    assert_eq!(row.display_name, None);
}

/// Mutant: the display-name stamp skipped — the caller's name is the birth
/// title and the row's display name, not just wire metadata.
#[test]
fn a_display_name_becomes_the_birth_title() {
    let (state, owner) = road_state("create-title-named", "S-1-5-21-create-named");
    let meta = SessionCreateMeta {
        display_name: Some("Night shell".to_string()),
        ..SessionCreateMeta::default()
    };
    create(
        &state,
        &owner,
        None,
        SessionKind::Terminal,
        None,
        Some(refused_spawn_command()),
        &None,
        &meta,
    )
    .expect_err("the fake binary refuses the spawn");
    let row = birth_row(&state, "Night shell");
    assert_eq!(row.display_name.as_deref(), Some("Night shell"));
}

/// Mutant: the agent-kind title default — an unnamed agent birth is titled
/// "Agent", not "Terminal", whatever family it started on.
#[test]
fn an_unnamed_agent_birth_is_titled_agent() {
    let (state, owner) = road_state("create-title-agent", "S-1-5-21-create-agent");
    let error = create(
        &state,
        &owner,
        None,
        SessionKind::Codex,
        Some("codex".to_string()),
        Some(refused_spawn_command()),
        &None,
        &SessionCreateMeta::default(),
    )
    .expect_err("the fake binary refuses the spawn");
    assert_eq!(error.code, ErrorCode::Io);
    let row = birth_row(&state, "Agent");
    assert_eq!(row.provider.as_deref(), Some("codex"));
}

/// Mutant: `note_pending_child` skipped on the create road — a create with
/// `creation_pending` parks its reservation marker; one without notes none.
#[test]
fn a_pending_agent_child_is_noted_when_creation_is_pending() {
    let (state, owner) = road_state("create-pending", "S-1-5-21-create-pending");
    let pending_meta = SessionCreateMeta {
        creation_pending: true,
        reservation: Some(4242),
        ..SessionCreateMeta::default()
    };
    let created = create(
        &state,
        &owner,
        None,
        SessionKind::Terminal,
        None,
        Some(echo_command()),
        &None,
        &pending_meta,
    )
    .expect("the echo spawn succeeds");
    {
        let table = state
            .sessions
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let marker = table
            .pending_children
            .get(&created.id)
            .expect("the pending marker");
        assert_eq!(marker.1, 4242, "the marker carries the reservation");
    }
    let plain = create(
        &state,
        &owner,
        None,
        SessionKind::Terminal,
        None,
        Some(echo_command()),
        &None,
        &SessionCreateMeta::default(),
    )
    .expect("the second echo spawn succeeds");
    let table = state
        .sessions
        .creations
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert!(
        !table.pending_children.contains_key(&plain.id),
        "a create without creation_pending notes no marker"
    );
}

/// Mutant: `clear_pending_child` dropped from the spawn-failure arm — a
/// failed spawn must release the marker its start noted.
#[test]
fn a_failed_spawn_clears_the_pending_child_it_noted() {
    let (state, owner) = road_state("create-pending-clear", "S-1-5-21-create-clear");
    let meta = SessionCreateMeta {
        creation_pending: true,
        reservation: Some(777),
        session_id: Some("s.create-clear.1".to_string()),
        ..SessionCreateMeta::default()
    };
    create(
        &state,
        &owner,
        None,
        SessionKind::Terminal,
        None,
        Some(refused_spawn_command()),
        &None,
        &meta,
    )
    .expect_err("the fake binary refuses the spawn");
    let table = state
        .sessions
        .creations
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert!(
        !table.pending_children.contains_key("s.create-clear.1"),
        "the failed spawn released the pending marker"
    );
}

/// Mutant: the roster invalidation lifted away from the journal row — the
/// birth write must drop the cached roster so the next read sees the row.
#[test]
fn the_birth_row_invalidates_the_journal_roster() {
    let (state, owner) = road_state("create-roster", "S-1-5-21-create-roster");
    state.sessions.journal_roster().expect("warm the roster");
    assert!(
        state
            .sessions
            .journal_roster
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some(),
        "the read warmed the cache"
    );
    create(
        &state,
        &owner,
        None,
        SessionKind::Terminal,
        None,
        Some(refused_spawn_command()),
        &None,
        &SessionCreateMeta::default(),
    )
    .expect_err("the fake binary refuses the spawn");
    assert!(
        state
            .sessions
            .journal_roster
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_none(),
        "the birth row dropped the cached roster"
    );
}

/// Mutant: the failure-arm health record dropped — a provider or pipe
/// failure (any non-InvalidRequest) must measure the provider's health.
#[test]
fn a_provider_spawn_failure_measures_provider_health() {
    let (state, owner) = road_state("create-health-io", "S-1-5-21-create-health");
    let error = create(
        &state,
        &owner,
        None,
        SessionKind::Codex,
        Some("codex".to_string()),
        Some(refused_spawn_command()),
        &None,
        &SessionCreateMeta::default(),
    )
    .expect_err("the fake binary refuses the spawn");
    assert_eq!(error.code, ErrorCode::Io);
    let health = state.provider_health("codex");
    assert!(
        health.starts_with("failed:"),
        "a spawn failure measures health: {health}"
    );
}

/// Mutant: health recorded before the `spawn_failure_is_provider_health`
/// gate — a profile's own InvalidRequest refusal must leave the provider's
/// health untouched ("unknown"), or healthy providers read unhealthy.
#[test]
fn a_profile_refusal_measures_no_provider_health() {
    let (state, owner) = road_state("create-health-gate", "S-1-5-21-create-gate");
    let error = state
        .sessions
        .create_with_provider_env(
            &state,
            &owner,
            None,
            SessionKind::Codex,
            Some("codex".to_string()),
            ProfileDelivery::for_request(Some("not-a-codex-mode".to_string())),
            Some(refused_spawn_command()),
            &None,
            None,
            &SessionCreateMeta::default(),
        )
        .expect_err("the unknown mode refuses the create");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(
        state.provider_health("codex"),
        "unknown",
        "a profile refusal says nothing about the provider"
    );
}

/// Mutant: the `reservation.expect` quietly demoted to a no-marker note —
/// a creation_pending child without a reservation is the bug the panic
/// names, and it must stay loud rather than note an ownerless marker.
#[test]
fn a_pending_child_without_a_reservation_panics() {
    let (state, owner) = road_state("create-reservation", "S-1-5-21-create-reserve");
    let meta = SessionCreateMeta {
        creation_pending: true,
        ..SessionCreateMeta::default()
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        create(
            &state,
            &owner,
            None,
            SessionKind::Terminal,
            None,
            Some(refused_spawn_command()),
            &None,
            &meta,
        )
    }));
    let payload = result.expect_err("the missing reservation must panic");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&'static str>()
                .map(|s| (*s).to_string())
        })
        .expect("a string panic payload");
    assert!(message.contains("reservation"), "{message}");
    // The panic is after the durable boundary. These assertions pin what that
    // position costs today — a live row nothing will end, no live entry, no
    // noted marker — so moving the guard without changing them is a red test,
    // not a silent change of the cost.
    let row = birth_row(&state, "Terminal");
    assert_eq!(
        row.status,
        PersistStatus::Live,
        "the birth row stays live: nothing ends it on the panic path"
    );
    assert!(
        !state
            .sessions
            .inner
            .lock()
            .expect("registry")
            .contains_key(&row.id),
        "the panic lands before the spawn, so no live entry exists"
    );
    let creations = state
        .sessions
        .creations
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert!(
        !creations.pending_children.contains_key(&row.id),
        "the guard fires before the marker is noted"
    );
}
