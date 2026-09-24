//! Direct unit tests of the phases in `session_create.rs`: each phase is
//! called on its own, so a phase that is wrong fails here even before the
//! through-the-road characterisation in `session_create_tests.rs` notices.

use std::path::PathBuf;

use devboule_protocol::{
    ErrorCode, OwnerId, SessionKind, SessionOrigin, WireError, WorkspaceIsolation,
};

use crate::journal::PersistStatus;
use crate::peer_policy::PeerRole;
use crate::profile_delivery::ProfileDelivery;
use crate::server::ServerState;

use super::session_create::{birth_stamps, build_birth_record};
use super::{PtyCommand, SessionCreateMeta};

fn test_owner(user: &str) -> OwnerId {
    OwnerId::new(user, "create-phase-client").expect("owner")
}

fn command_in(cwd: PathBuf) -> PtyCommand {
    PtyCommand::new("whatever-but-never-spawned", Vec::new(), cwd, Vec::new())
}

/// Mutant: the carried origin demoted below the connection, or the kind
/// title default flipped — the pure stamps answer each input faithfully.
#[test]
fn birth_stamps_answer_meta_then_connection_then_kind() {
    let meta = SessionCreateMeta::default();
    let (origin, title) = birth_stamps(&meta, &None, &SessionKind::Terminal);
    assert_eq!(origin, SessionOrigin::local());
    assert_eq!(title, "Terminal");

    let carried = SessionCreateMeta {
        origin: Some(SessionOrigin::peer("dev-a", PeerRole::Daemon)),
        ..SessionCreateMeta::default()
    };
    let (origin, title) = birth_stamps(&carried, &None, &SessionKind::Acp);
    assert_eq!(origin, SessionOrigin::peer("dev-a", PeerRole::Daemon));
    assert_eq!(title, "Agent");

    let named = SessionCreateMeta {
        display_name: Some("Called by name".to_string()),
        ..SessionCreateMeta::default()
    };
    let (origin, title) = birth_stamps(&named, &None, &SessionKind::Acp);
    assert_eq!(origin, SessionOrigin::local());
    assert_eq!(title, "Called by name");
}

fn resolved(
    id: &str,
    kind: SessionKind,
    session_provider: Option<&str>,
    cwd: PathBuf,
) -> super::session_create::ResolvedCreation {
    super::session_create::ResolvedCreation {
        id: id.to_string(),
        kind,
        command: command_in(cwd.clone()),
        session_provider: session_provider.map(str::to_string),
        row_cwd: cwd,
    }
}

/// Mutant: a birth fact dropped from the row or the metadata — the pure
/// builder stamps the same facts on both, and defaults the child's context
/// to its own id only when no inherited context arrived.
#[test]
fn build_birth_record_stamps_the_birth_facts_on_row_and_metadata() {
    let owner = test_owner("S-1-5-21-phase-birth");
    let meta = SessionCreateMeta {
        display_name: Some("Birth facts".to_string()),
        created_by: Some("s.creator.1".to_string()),
        depth: 2,
        profile_id: Some("profile-7".to_string()),
        context_id: Some("s.root.9".to_string()),
        ..SessionCreateMeta::default()
    };
    let resolved_child = resolved(
        "s.child.1",
        SessionKind::Acp,
        Some("codex"),
        crate::test_dirs::test_temp_dir("devboule-phase-cwd"),
    );
    let (record, metadata, generation) = build_birth_record(
        &resolved_child,
        &owner,
        Some("w.1".to_string()),
        &ProfileDelivery::none(),
        &meta,
        SessionOrigin::local(),
        "Birth facts".to_string(),
    );
    assert_eq!(record.id, "s.child.1");
    assert_eq!(record.created_by.as_deref(), Some("s.creator.1"));
    assert_eq!(record.depth, Some(2));
    assert_eq!(record.profile_id.as_deref(), Some("profile-7"));
    assert_eq!(record.context_id.as_deref(), Some("s.root.9"));
    assert_eq!(record.status, PersistStatus::Live);
    assert!(record.overlay.is_some(), "the overlay defaults, not None");
    assert_eq!(generation, record.generation);
    assert_eq!(metadata.id, "s.child.1");
    assert_eq!(metadata.context_id.as_deref(), Some("s.root.9"));
    assert_eq!(metadata.title, "Birth facts");
    assert!(!metadata.resumable, "born live, so resume is refused");

    let orphan = SessionCreateMeta::default();
    let resolved_own = resolved(
        "s.own.1",
        SessionKind::Terminal,
        None,
        crate::test_dirs::test_temp_dir("devboule-phase-cwd"),
    );
    let (record, metadata, _) = build_birth_record(
        &resolved_own,
        &owner,
        None,
        &ProfileDelivery::none(),
        &orphan,
        SessionOrigin::local(),
        "Terminal".to_string(),
    );
    assert_eq!(record.context_id.as_deref(), Some("s.own.1"));
    assert_eq!(metadata.context_id.as_deref(), Some("s.own.1"));
}

/// Mutant: the cwd override dropped from resolution, or the family's
/// provider stamp lost — resolution returns the override cwd and the
/// family's own stamp for the session.
#[test]
fn resolve_creation_inputs_honours_the_cwd_override_and_family_stamp() {
    let state = ServerState::new("create-phase-resolve".to_string());
    let owner = test_owner("S-1-5-21-phase-resolve");
    let cwd = crate::test_dirs::test_temp_dir("devboule-phase-override");
    let meta = SessionCreateMeta {
        cwd: Some(cwd.clone()),
        ..SessionCreateMeta::default()
    };
    let resolved = state
        .sessions
        .resolve_creation_inputs(
            &owner,
            None,
            SessionKind::Terminal,
            None,
            None,
            Some(command_in(PathBuf::from("Z:\\never"))),
            &meta,
        )
        .expect("resolution with no workspace lookup");
    assert_eq!(resolved.command.cwd, cwd);
    let parts: Vec<&str> = resolved.id.split('.').collect();
    assert_eq!(parts.len(), 3, "a minted id: {}", resolved.id);
    assert_eq!(
        resolved.session_provider, None,
        "a terminal carries no provider stamp"
    );
    let named = state
        .sessions
        .resolve_creation_inputs(
            &owner,
            None,
            SessionKind::Codex,
            Some("codex".to_string()),
            None,
            Some(command_in(PathBuf::from("Z:\\never"))),
            &SessionCreateMeta::default(),
        )
        .expect("resolution of a named provider");
    assert_eq!(
        named.session_provider.as_deref(),
        Some("codex"),
        "the family stamps its own id on the session"
    );
}

/// Mutant: the `meta.cwd` override copied into the command as it arrived —
/// a recovered row's stored `\\?\` cwd must be converted at the hand-off,
/// or the recovered child's cmd.exe runs in `C:\Windows` while the row
/// keeps the spelling it was written with.
#[test]
fn a_carried_verbatim_cwd_is_converted_for_the_child_and_recorded_raw() {
    let state = ServerState::new("create-phase-carried-cwd".to_string());
    let owner = test_owner("S-1-5-21-phase-carried-cwd");
    let stored = std::fs::canonicalize(crate::test_dirs::test_temp_dir(
        "devboule-phase-carried-cwd",
    ))
    .expect("canonical fixture");
    let stored = stored.to_string_lossy().into_owned();
    assert!(
        stored.starts_with(r"\\?\"),
        "the fixture is verbatim: {stored}"
    );
    let meta = SessionCreateMeta {
        cwd: Some(stored.clone().into()),
        ..SessionCreateMeta::default()
    };
    let resolved = state
        .sessions
        .resolve_creation_inputs(
            &owner,
            None,
            SessionKind::Terminal,
            None,
            None,
            Some(command_in(PathBuf::from("Z:\\never"))),
            &meta,
        )
        .expect("resolution with no workspace lookup");
    let plain = crate::workspace::plain_path(&stored);
    assert_eq!(
        resolved.command.cwd,
        PathBuf::from(&plain),
        "the child takes the plain spelling at the hand-off"
    );
    let (record, _metadata, _) = build_birth_record(
        &resolved,
        &owner,
        None,
        &ProfileDelivery::none(),
        &meta,
        SessionOrigin::local(),
        "Terminal".to_string(),
    );
    assert_eq!(
        record.cwd.as_deref(),
        Some(stored.as_str()),
        "the row records the stored spelling, not the child's"
    );
}

/// Mutant: the birth row recording the child's plain cwd — the row keeps
/// the workspace's stored verbatim spelling while the child and the wire
/// metadata take the plain one.
#[test]
fn the_birth_row_keeps_the_stored_verbatim_cwd_while_the_child_gets_plain() {
    let state = ServerState::new("create-phase-row-cwd".to_string());
    let owner = test_owner("S-1-5-21-phase-row-cwd");
    let dir = crate::test_dirs::test_temp_dir("devboule-phase-row-cwd");
    let stored = std::fs::canonicalize(&dir).expect("canonical fixture");
    let stored = stored.to_string_lossy().into_owned();
    assert!(
        stored.starts_with(r"\\?\"),
        "the fixture is verbatim: {stored}"
    );
    let project = state
        .sessions
        .project_add(&dir.to_string_lossy())
        .expect("project");
    let workspace = state
        .sessions
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("workspace");
    let meta = SessionCreateMeta::default();
    let resolved = state
        .sessions
        .resolve_creation_inputs(
            &owner,
            Some(&workspace.id),
            SessionKind::Terminal,
            None,
            None,
            Some(command_in(PathBuf::from("Z:\\never"))),
            &meta,
        )
        .expect("resolution against the real workspace");
    let plain = crate::workspace::plain_path(&stored);
    assert_eq!(
        resolved.command.cwd,
        PathBuf::from(&plain),
        "the child starts in the plain spelling"
    );
    let (record, metadata, _) = build_birth_record(
        &resolved,
        &owner,
        Some(workspace.id.clone()),
        &ProfileDelivery::none(),
        &meta,
        SessionOrigin::local(),
        "Terminal".to_string(),
    );
    assert_eq!(
        record.cwd.as_deref(),
        Some(stored.as_str()),
        "the row records the workspace's stored verbatim path"
    );
    assert_eq!(
        metadata.cwd.as_deref(),
        Some(plain.as_str()),
        "the wire metadata carries the plain spelling"
    );
}

/// Mutant: the pending note gated on anything but `creation_pending`, or
/// the reservation dropped from the marker.
#[test]
fn note_pending_child_if_creation_pending_notes_exactly_when_pending() {
    let state = ServerState::new("create-phase-note".to_string());
    let pending = SessionCreateMeta {
        creation_pending: true,
        reservation: Some(31),
        ..SessionCreateMeta::default()
    };
    state
        .sessions
        .note_pending_child_if_creation_pending("s.phase-note.1", &pending);
    let plain = SessionCreateMeta::default();
    state
        .sessions
        .note_pending_child_if_creation_pending("s.phase-note.2", &plain);
    let table = state
        .sessions
        .creations
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert_eq!(
        table.pending_children.get("s.phase-note.1").map(|m| m.1),
        Some(31)
    );
    assert!(!table.pending_children.contains_key("s.phase-note.2"));
}

/// Mutant: the failure arm's order or its health gate — the clear happens,
/// and a provider-or-pipe failure measures health while a profile's own
/// InvalidRequest refusal leaves it alone.
#[test]
fn fail_spawn_clears_and_measures_only_provider_health_failures() {
    let state = ServerState::new("create-phase-fail".to_string());
    state.sessions.note_pending_child("s.phase-fail.1", 5);
    let error = state.sessions.fail_spawn(
        &state,
        "s.phase-fail.1",
        0,
        Some("codex"),
        WireError::new(ErrorCode::Io, "ACP stdio failed: broken pipe"),
    );
    assert_eq!(error.code, ErrorCode::Io, "the refusal travels unchanged");
    let table = state
        .sessions
        .creations
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert!(!table.pending_children.contains_key("s.phase-fail.1"));
    drop(table);
    let measured = state.provider_health("codex");
    assert!(measured.starts_with("failed:"), "{measured}");

    state.sessions.fail_spawn(
        &state,
        "s.phase-fail.2",
        0,
        Some("codex"),
        WireError::new(ErrorCode::InvalidRequest, "the profile contradicts itself"),
    );
    assert_eq!(
        state.provider_health("codex"),
        measured,
        "a profile refusal does not overwrite the measurement"
    );
}
