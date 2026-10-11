use super::*;

use devboule_protocol::SessionKind;

#[test]
fn a_session_cannot_start_in_a_workspace_while_it_is_being_archived() {
    let (state, project, _own, _dir) = setup("archive-create-during-archive");
    let (target, path) = add_worktree(&state, &project, "archive-create-during-archive");
    let archiving = state
        .sessions
        .mark_workspace_archiving(&target)
        .expect("the archive takes the mark");
    let refused = state.sessions.create(
        &state,
        &owner(),
        Some(target.clone()),
        SessionKind::Terminal,
        None,
        None,
        None,
        &None,
        None,
    );
    let Err(error) = refused else {
        panic!("a session starts in a workspace being archived");
    };
    assert_eq!(error.message, "Workspace is being archived.");
    drop(archiving);
    assert!(state
        .sessions
        .workspace_creation_guard(Some(&target))
        .is_ok());
    assert!(path.is_dir(), "an archive keeps the folder");
}
