//! Every row owns sessions, so identity is carried by distinct titles —
//! never by merging rows or moving them off their id.

use std::collections::HashSet;
use std::sync::{Arc, Barrier};
use std::thread;

use crate::server::ServerState;
use devboule_protocol::{ErrorCode, WorkspaceIsolation};

fn project_folder(tag: &str) -> std::path::PathBuf {
    let dir = crate::test_dirs::test_temp_dir(&format!("devboule-{tag}"));
    let root = dir.join("devboule-v2");
    std::fs::create_dir_all(&root).expect("project folder");
    dir
}

#[test]
fn a_second_local_row_on_one_folder_gets_its_own_default_title() {
    let dir = project_folder("workspace-identity");
    let root = dir.join("devboule-v2");
    let state = ServerState::new("workspace-identity".to_string());
    let project = state
        .sessions
        .project_add(root.to_str().expect("project path is UTF-8"))
        .expect("project row");

    let first = state
        .sessions
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("first local workspace");
    let second = state
        .sessions
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("second local workspace");

    assert_ne!(first.id, second.id, "each row keeps its own id");
    assert_eq!(
        first.title, "devboule-v2",
        "the first local row keeps the folder's bare name"
    );
    assert_eq!(
        second.title, "devboule-v2 2",
        "a later local row on the same folder is numbered, not a second bare name"
    );
    assert_eq!(
        first.path, second.path,
        "both rows still point at the project folder"
    );
    assert_eq!(first.isolation, WorkspaceIsolation::Local);
    assert_eq!(second.isolation, WorkspaceIsolation::Local);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rename_stores_the_title_on_the_row_and_leaves_its_id_alone() {
    let dir = project_folder("workspace-rename");
    let root = dir.join("devboule-v2");
    let state = ServerState::new("workspace-rename".to_string());
    let project = state
        .sessions
        .project_add(root.to_str().expect("project path is UTF-8"))
        .expect("project row");
    let first = state
        .sessions
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("first local workspace");
    let second = state
        .sessions
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("second local workspace");

    let renamed = state
        .sessions
        .workspace_set_title(&second.id, "  night build  ")
        .expect("rename");

    assert_eq!(
        renamed.id, second.id,
        "a rename moves the title, never the id the row's sessions hang from"
    );
    assert_eq!(
        renamed.title, "night build",
        "the daemon stores the trimmed title"
    );
    assert_eq!(renamed.path, second.path);
    let listed = state
        .sessions
        .workspaces_list(&project.id)
        .expect("list after rename");
    assert_eq!(
        listed
            .iter()
            .find(|row| row.id == first.id)
            .map(|row| row.title.as_str()),
        Some("devboule-v2"),
        "the other row keeps its own title"
    );
    assert_eq!(
        listed
            .iter()
            .find(|row| row.id == second.id)
            .map(|row| row.title.as_str()),
        Some("night build"),
        "the new title is the one the journal holds"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rename_the_title_rule_refuses_leaves_the_row_as_it_was() {
    let dir = project_folder("workspace-rename-refused");
    let root = dir.join("devboule-v2");
    let state = ServerState::new("workspace-rename-refused".to_string());
    let project = state
        .sessions
        .project_add(root.to_str().expect("project path is UTF-8"))
        .expect("project row");
    let workspace = state
        .sessions
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("local workspace");

    let error = state
        .sessions
        .workspace_set_title(&workspace.id, "   ")
        .expect_err("an empty title must be refused");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        error.message.contains("required"),
        "the refusal says why: {}",
        error.message
    );
    let listed = state
        .sessions
        .workspaces_list(&project.id)
        .expect("list after refusal");
    assert_eq!(listed[0].title, "devboule-v2", "nothing was stored");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rename_of_a_row_the_journal_does_not_hold_is_refused() {
    let dir = project_folder("workspace-rename-missing");
    let state = ServerState::new("workspace-rename-missing".to_string());

    let error = state
        .sessions
        .workspace_set_title("w.missing", "night build")
        .expect_err("an unknown row must be refused");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("w.missing"), "{}", error.message);

    let _ = std::fs::remove_dir_all(&dir);
}

/// A stress check, not a proof: six barrier-synchronized pairs of real
/// threads against one project — every title it stores must stay distinct.
#[test]
fn a_thread_stress_check_keeps_every_create_title_distinct() {
    let dir = project_folder("workspace-identity-race");
    let root = dir.join("devboule-v2");
    let state = ServerState::new("workspace-identity-race".to_string());
    let project = state
        .sessions
        .project_add(root.to_str().expect("project path is UTF-8"))
        .expect("project row");
    state
        .sessions
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("seed row");

    let mut titles = Vec::new();
    for _ in 0..6 {
        let barrier = Arc::new(Barrier::new(2));
        let creators: Vec<_> = (0..2)
            .map(|_| {
                let state = Arc::clone(&state);
                let project_id = project.id.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    state
                        .sessions
                        .workspace_create(&project_id, WorkspaceIsolation::Local, None)
                        .expect("concurrent create")
                        .title
                })
            })
            .collect();
        for creator in creators {
            titles.push(creator.join().expect("creator thread"));
        }
    }

    let unique: HashSet<&String> = titles.iter().collect();
    assert_eq!(
        unique.len(),
        titles.len(),
        "concurrent creates shared a default title: {titles:?}"
    );
    assert_eq!(titles.len(), 12);

    let _ = std::fs::remove_dir_all(&dir);
}
