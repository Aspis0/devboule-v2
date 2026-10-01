//! The roads that hold the workspace creation gate. A real create must count
//! its workspace from its birth row until the session is registered: a delete
//! landing in between would find no live session to refuse on and remove the
//! checkout under a session about to spawn there. A resume must take the same
//! guard before it starts a provider.

use std::sync::Mutex;

use super::session_resume_fixture::{acp_row, ResumeFixture};
use super::*;

/// The checkpoint hook asks for the archive mark from inside the create, at
/// the birth row and again once the session is registered: both times a
/// session is starting, so both times the mark must refuse.
#[test]
fn a_create_counts_its_workspace_from_the_birth_row_until_registered() {
    let state = ServerState::new("gate-road-create".to_string());
    let owner = OwnerId::new("s-1-5-21-gate-road", "gate-road-create").expect("owner");
    let dir = state.sessions.runtime_dir().to_path_buf();
    let journal = Arc::clone(state.sessions.journal.as_ref().expect("journal"));
    let project_path = dir.join("Project");
    std::fs::create_dir(&project_path).expect("project folder");
    let project = crate::workspace::project_record(
        project_path.to_str().expect("project path is valid UTF-8"),
    )
    .expect("project record");
    let project = journal.project_add(project).expect("persist project");
    let workspace = journal
        .workspace_create(crate::workspace::local_workspace_record(&project))
        .expect("persist workspace")
        .id;

    let checkpoints = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&checkpoints);
    let target = workspace.clone();
    state
        .sessions
        .set_create_gate_checkpoint_hook(Arc::new(move |registry: &SessionRegistry| {
            let refusal = registry
                .mark_workspace_archiving(&target)
                .err()
                .map(|error| error.message);
            seen.lock().expect("checkpoints").push(refusal);
        }));
    let session = state
        .sessions
        .create(
            &state,
            &owner,
            Some(workspace.clone()),
            SessionKind::Terminal,
            None,
            None,
            None,
            &None,
            None,
        )
        .expect("the terminal create");

    assert_eq!(
        *checkpoints.lock().expect("checkpoints"),
        vec![Some(SESSION_STARTING_MESSAGE.to_string()); 2],
        "the create's count must span its birth row and its registration"
    );
    assert!(
        state.sessions.mark_workspace_archiving(&workspace).is_ok(),
        "the count ends with the create"
    );
    let _ = state.sessions.close(&session.id, &owner, &None);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_resume_into_a_workspace_being_archived_is_refused() {
    let fixture = ResumeFixture::new("gate-road-archiving");
    let id = fixture.id("gate-road-archiving");
    let workspace = "w.gate-road-archiving";
    let mut row = acp_row(&id, &fixture.owner, "stub-session");
    row.workspace_id = Some(workspace.to_string());
    fixture.write_row(row);
    let archiving = fixture
        .registry()
        .mark_workspace_archiving(workspace)
        .expect("the archive mark");

    let error = fixture
        .resume(&id, &fixture.conn())
        .expect_err("a resume into a workspace being archived must refuse");
    assert_eq!(error.message, "Workspace is being archived.");

    drop(archiving);
    fixture.finish();
}
