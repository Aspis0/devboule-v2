use super::*;

use devboule_protocol::SessionKind;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[test]
fn creation_during_the_close_loop_is_refused_and_archive_clears_the_mark() {
    let (state, project, _own, _dir) = setup("archive-create-during-close");
    let (target, path) = add_worktree(&state, &project, "archive-create-during-close");
    crate::session::insert_test_live_session_in_workspace(
        &state.sessions,
        "archive-create-during-close-existing",
        owner(),
        SessionKind::Acp,
        &target,
    );
    let create_result = Arc::new(Mutex::new(None));
    let hook_result = Arc::clone(&create_result);
    let archive_state = Arc::clone(&state);
    let archive_target = target.clone();
    let archive = std::thread::spawn(move || {
        super::super::super::archive_workspace_with_close_hook(
            &archive_state,
            &archive_state.mcp,
            "archive-create-during-close",
            &owner(),
            &archive_target,
            || {
                let result = archive_state.sessions.create(
                    &archive_state,
                    &owner(),
                    Some(archive_target.clone()),
                    SessionKind::Terminal,
                    None,
                    None,
                    None,
                    &None,
                );
                let result = match result {
                    Ok(session) => {
                        let _ = archive_state.sessions.close(&session.id, &owner(), &None);
                        "creation succeeded".to_string()
                    }
                    Err(error) => error.message,
                };
                *hook_result.lock().expect("creation result") = Some(result);
            },
        )
    });
    let broker = state
        .sessions
        .live_runtime("archive-create-during-close", &owner())
        .expect("caller runtime")
        .permission_broker()
        .expect("permission broker");
    let started = Instant::now();
    let card_id = loop {
        if let Some(id) = broker.test_pending_ids().pop() {
            break id;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "archive card not raised"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    broker
        .test_answer(&card_id, PermissionOutcome::AllowOnce, "once")
        .expect("approve archive");
    let result = archive
        .join()
        .expect("archive call")
        .expect("archive succeeds");
    assert_eq!(
        create_result.lock().expect("creation result").as_deref(),
        Some("Workspace is being archived.")
    );
    assert_eq!(
        result["closedSessionIds"][0],
        "archive-create-during-close-existing"
    );
    assert!(!path.exists());
    assert!(state
        .sessions
        .workspace_creation_guard(Some(&target))
        .is_ok());
}
