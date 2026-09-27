//! Provider on/off: the spawn, probe and vocabulary doors a switched-off
//! provider closes, and the live sessions it never touches.
//!
//! The switch lives beside the tool policies and is read at the moment of
//! each decision. These tests pin the doors: the create road (wire and,
//! through the same function, MCP children and recovery), resume, and the
//! running session the switch must strand.

use super::session_resume_fixture::{acp_row, take_bystander_slot, ResumeFixture};
use super::tests::{
    attach_live_agent_for_test, insert_live_agent_with_writer, test_owner, RecordingWriter,
};
use super::*;
use std::sync::Mutex;

fn switch_state(tag: &str) -> (std::path::PathBuf, Arc<ServerState>) {
    let dir = crate::test_dirs::test_temp_dir(&format!("devboule-switch-{tag}"));
    let state = ServerState::with_paths(
        "test-instance".to_string(),
        crate::paths::RuntimePaths::from_dir(dir.clone()),
    )
    .expect("state");
    (dir, state)
}

fn echo_command() -> PtyCommand {
    PtyCommand::new(
        "cmd.exe",
        vec!["/c".to_string(), "echo switch-road".to_string()],
        crate::test_dirs::test_temp_dir("devboule-switch-cwd"),
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
fn create(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    kind: SessionKind,
    provider: Option<String>,
    command: Option<PtyCommand>,
) -> Result<Session, WireError> {
    state.sessions.create_with_provider_env(
        state,
        owner,
        None,
        kind,
        provider,
        crate::profile_delivery::ProfileDelivery::none(),
        command,
        &None,
        None,
        &SessionCreateMeta::default(),
    )
}

fn journal_rows(state: &Arc<ServerState>) -> Vec<crate::journal::SessionRecord> {
    state
        .sessions
        .journal
        .as_ref()
        .expect("the test state has a journal")
        .list()
        .expect("journal rows")
}

#[test]
fn a_switched_off_provider_refuses_the_create_road_before_the_birth_door() {
    let (dir, state) = switch_state("create-refused");
    let owner = test_owner("switch-create", "c1");
    state
        .provider_switches
        .set("claude", false)
        .expect("the switch lands");
    let error = create(
        &state,
        &owner,
        SessionKind::Claude,
        None,
        Some(echo_command()),
    )
    .expect_err("a switched-off provider cannot be spawned");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        error.message.contains("claude") && error.message.contains("Providers"),
        "one plain sentence naming the provider and the place: {error:?}"
    );
    assert!(
        journal_rows(&state).is_empty(),
        "the refusal fires before the birth door; no row may exist"
    );
    state.sessions.journal.as_ref().expect("journal").shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unknown_provider_id_passes_the_switch_open() {
    let (dir, state) = switch_state("create-unknown");
    state
        .provider_switches
        .set("claude", false)
        .expect("the store has a real disabled entry");
    let canonicalized =
        crate::provider_switches::refuse_if_disabled(&state.provider_switches, "CLAUDE")
            .expect_err("a case alias of a disabled provider must be refused");
    assert_eq!(canonicalized.code, ErrorCode::InvalidRequest);
    assert!(canonicalized.message.contains("claude"));
    // An unrelated id remains open even when the store contains a disabled row.
    crate::provider_switches::refuse_if_disabled(&state.provider_switches, "test-agent")
        .expect("an unknown provider is not switched off");
    state.sessions.journal.as_ref().expect("journal").shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_switched_off_provider_refuses_resume_before_any_slot_moves() {
    let fixture = ResumeFixture::new("switch-resume");
    let id = fixture.id("switch-resume");
    let mut row = acp_row(&id, &fixture.owner, "stub-session");
    row.provider = Some("claude".to_string());
    fixture.write_row(row);
    fixture
        .state
        .provider_switches
        .set("claude", false)
        .expect("the switch lands");
    take_bystander_slot(&fixture.state);

    let error = fixture
        .resume(&id, &fixture.conn())
        .expect_err("an ended session of a switched-off provider stays ended");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        error.message.contains("claude"),
        "the refusal names the switch, not the spawn: {error:?}"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        1,
        "a refusal before staging takes no slot"
    );
    fixture.finish();
}

#[test]
fn a_running_session_keeps_running_after_its_provider_is_switched_off() {
    let (dir, state) = switch_state("keeps-running");
    let owner = test_owner("switch-live", "c1");
    let id = "s.switch.1";
    let runtime = insert_live_agent_with_writer(
        &state.sessions,
        id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    let conn = attach_live_agent_for_test(&runtime, id, 71);
    state
        .provider_switches
        .set("claude", false)
        .expect("the switch lands");
    state
        .sessions
        .send_with_subscription(id, conn.id, "still working", &[], &[], &owner, &conn)
        .expect("a live session is never re-checked");
    journal_shutdown(&state);
    let _ = std::fs::remove_dir_all(&dir);
}

fn journal_shutdown(state: &Arc<ServerState>) {
    state.sessions.journal.as_ref().expect("journal").shutdown();
}
