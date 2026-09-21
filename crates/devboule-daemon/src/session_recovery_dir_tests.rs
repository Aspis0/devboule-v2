//! The folder question, split out of `session_recovery_tests.rs`: which
//! directory the replacement session is launched in, and what the sentence the
//! human reads says about it. Two roads reach the recovery — a workspace whose
//! **row** was deleted while its folder stayed on disk, and a folder that is
//! really gone — and they must not read the same to the person who clicked.

use super::session_resume_fixture::{acp_row, take_bystander_slot, AcpEnv, ResumeFixture};
use super::*;
use devboule_protocol::{UserMessageAuthor, UserMessageKind};

fn user(text: &str) -> SessionEvent {
    SessionEvent::AgentUserMessage {
        message_id: None,
        text: text.to_string(),
        author: UserMessageAuthor::Human,
        message_kind: UserMessageKind::Composer,
    }
}

/// The notice the replacement journaled for itself, read back the way the panel
/// reads it after a restart.
fn notice_of(fixture: &ResumeFixture, id: &str) -> String {
    fixture.journal().flush().expect("flush");
    fixture
        .journal()
        .replay(id)
        .expect("the new session's transcript")
        .events
        .iter()
        .find_map(|event| match event {
            SessionEvent::SessionNotice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .expect("the replacement says why it exists")
}

/// A workspace row is deleted while the folder it names stays on disk —
/// `workspace_delete` never checks whether a session still points at it — and
/// the directory the row of the session recorded is then the one fact that
/// brings the conversation back where it worked. Before this test, the
/// replacement was created with no directory at all: a conversation that could
/// have continued in the right folder started in the daemon's own.
///
/// Mutants: `meta.cwd` dropped (the replacement starts wherever the daemon
/// stands, and the assertion on `session.cwd` dies); the kept branch of the
/// notice turned into the gone one (the notice assertion dies). The `is_dir`
/// guard is not this test's mutant — the folder is fine here.
#[test]
fn a_recovered_session_starts_in_the_directory_the_old_one_recorded() {
    let fixture = ResumeFixture::new("recover-kept");
    let id = fixture.id("recover-kept");
    let _env = AcpEnv::stub(&[]);
    let _broker = fixture.state.mcp.start(&fixture.state).expect("MCP server");
    let work = fixture.dir.join("still-there");
    std::fs::create_dir_all(&work).expect("the directory the session worked in");
    let project = crate::workspace::project_record(work.to_str().expect("utf-8 path"))
        .expect("project record");
    let project = fixture.journal().project_add(project).expect("project row");
    let workspace = fixture
        .journal()
        .workspace_create(crate::workspace::local_workspace_record(&project))
        .expect("workspace row");
    let mut row = acp_row(&id, &fixture.owner, "handle-kept");
    row.workspace_id = Some(workspace.id.clone());
    row.cwd = Some(work.to_string_lossy().into_owned());
    fixture.write_row(row);
    fixture.record_turn(&id, 1, &user("did you check the tests?"));
    fixture
        .journal()
        .workspace_delete(&workspace.id)
        .expect("the row goes, the folder stays");
    take_bystander_slot(&fixture.state);

    let session = fixture
        .resume(&id, &fixture.conn())
        .expect("the conversation is recovered into a new session");
    assert_ne!(session.id, id, "the row is gone, the session is new");
    let shown = crate::workspace::display_path(&work.to_string_lossy());
    assert_eq!(
        session.cwd.as_deref(),
        Some(shown.as_str()),
        "the replacement is launched in the directory the old session recorded"
    );
    assert_eq!(
        fixture.row(&session.id).cwd.as_deref(),
        Some(work.to_string_lossy().as_ref()),
        "and the new row records it raw, so a later reopen can check it again"
    );
    let notice = notice_of(&fixture, &session.id);
    assert!(
        notice.contains(&format!(
            "It starts in the directory the old session was launched in: {shown}."
        )),
        "the human is told where their new session works: {notice}"
    );
    let _ = fixture
        .state
        .sessions
        .close(&session.id, &fixture.owner, &None);
    fixture.finish();
}

/// The other half: the workspace row is there and its folder is gone, so there
/// is no directory to bring along. The sentence has to say so and name the
/// path, because the daemon's own reason names the **workspace id**
/// (`Workspace 'w-…' is unavailable: its folder is no longer available.`), and
/// nobody can look at a folder they were never told.
///
/// Mutants: the folder sentence dropped from the notice, or the path left out
/// of it (the assertion on the path dies); the `is_dir` guard dropped, so the
/// recorded path is handed to the create (the spawn is refused on a directory
/// that does not exist and the resume dies at the `expect`).
#[test]
fn a_recovered_session_whose_folder_is_gone_says_which_path_died() {
    let fixture = ResumeFixture::new("recover-gone-workspace");
    let id = fixture.id("recover-gone-workspace");
    let _env = AcpEnv::stub(&[]);
    let _broker = fixture.state.mcp.start(&fixture.state).expect("MCP server");
    let gone = fixture.dir.join("removed-worktree");
    std::fs::create_dir_all(&gone).expect("the directory the session worked in");
    let project = crate::workspace::project_record(gone.to_str().expect("utf-8 path"))
        .expect("project record");
    let project = fixture.journal().project_add(project).expect("project row");
    let workspace = fixture
        .journal()
        .workspace_create(crate::workspace::local_workspace_record(&project))
        .expect("workspace row");
    let mut row = acp_row(&id, &fixture.owner, "handle-gone");
    row.workspace_id = Some(workspace.id.clone());
    row.cwd = Some(gone.to_string_lossy().into_owned());
    fixture.write_row(row);
    fixture.record_turn(&id, 1, &user("did you check the tests?"));
    std::fs::remove_dir_all(&gone).expect("the directory goes");
    take_bystander_slot(&fixture.state);

    let session = fixture
        .resume(&id, &fixture.conn())
        .expect("the conversation is recovered into a new session");
    let shown = crate::workspace::display_path(&gone.to_string_lossy());
    assert!(
        session.workspace_id.is_none(),
        "the workspace is gone; the replacement carries none: {:?}",
        session.workspace_id
    );
    assert_ne!(
        session.cwd.as_deref(),
        Some(shown.as_str()),
        "the directory that died does not come along"
    );
    let notice = notice_of(&fixture, &session.id);
    assert!(
        notice.contains(&format!(
            "It has no folder of its own to work in: {shown} is gone."
        )),
        "the sentence names the path that is gone, not just the workspace id: {notice}"
    );
    assert!(
        notice.contains(&workspace.id),
        "the daemon's own reason still names the workspace it looked up: {notice}"
    );
    let _ = fixture
        .state
        .sessions
        .close(&session.id, &fixture.owner, &None);
    fixture.finish();
}
