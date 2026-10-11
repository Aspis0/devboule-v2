//! The archive and the removal the sidebar's menus send: a workspace row or a
//! project leaves the list, and the folder it names stays on disk.

use super::tests::tmp_delete_registry;
use super::*;

/// A project folder on disk, registered, with its local row: the shape every
/// archive and removal starts from.
fn project_with_local_row(
    dir: &std::path::Path,
    journal: &Journal,
) -> (
    std::path::PathBuf,
    crate::journal::ProjectRecord,
    crate::journal::WorkspaceRecord,
) {
    let project_path = dir.join("Project");
    std::fs::create_dir(&project_path).expect("project folder");
    let project = journal
        .project_add(
            crate::workspace::project_record(project_path.to_str().expect("utf-8 path"))
                .expect("project record"),
        )
        .expect("persist project");
    let local = journal
        .workspace_create(crate::workspace::local_workspace_record(&project))
        .expect("persist local workspace");
    (project_path, project, local)
}

fn listed_workspace_ids(journal: &Journal, project_id: &str) -> Vec<String> {
    journal
        .workspaces_list(project_id)
        .expect("workspace list")
        .into_iter()
        .map(|workspace| workspace.id)
        .collect()
}

#[test]
fn archiving_a_worktree_row_takes_it_off_the_list_and_keeps_its_checkout() {
    let (dir, registry, journal) = tmp_delete_registry();
    let (_, project, _local) = project_with_local_row(&dir, &journal);
    let checkout = dir.join("checkout");
    std::fs::create_dir(&checkout).expect("checkout folder");
    let worktree = journal
        .workspace_create(crate::workspace::worktree_workspace_record(
            &project, &checkout, "main",
        ))
        .expect("persist worktree");

    registry.workspace_archive(&worktree.id).expect("archive");

    assert!(
        !listed_workspace_ids(&journal, &project.id).contains(&worktree.id),
        "an archived row leaves the list"
    );
    assert!(checkout.is_dir(), "the checkout folder stays on disk");
    assert!(
        journal.workspace_get(&worktree.id).expect("get").is_some(),
        "the row stays in the journal, so its sessions keep their history"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn archiving_the_local_row_keeps_the_project_folder() {
    let (dir, registry, journal) = tmp_delete_registry();
    let (project_path, project, local) = project_with_local_row(&dir, &journal);

    registry.workspace_archive(&local.id).expect("archive");

    assert!(
        !listed_workspace_ids(&journal, &project.id).contains(&local.id),
        "the local row leaves the list"
    );
    assert!(project_path.is_dir(), "the project folder is never removed");

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn archiving_twice_and_archiving_an_unknown_row() {
    let (dir, registry, journal) = tmp_delete_registry();
    let (_, project, local) = project_with_local_row(&dir, &journal);

    registry
        .workspace_archive(&local.id)
        .expect("first archive");
    registry
        .workspace_archive(&local.id)
        .expect("a second archive changes nothing and is not an error");
    assert!(listed_workspace_ids(&journal, &project.id).is_empty());

    assert!(
        registry.workspace_archive("w.missing").is_err(),
        "an unknown workspace is refused, never reported as archived"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn removing_a_project_takes_it_off_the_list_and_adding_its_folder_lists_it_again() {
    let (dir, registry, journal) = tmp_delete_registry();
    let (project_path, project, local) = project_with_local_row(&dir, &journal);

    registry.project_remove(&project.id).expect("remove");

    assert!(
        !journal
            .projects_list()
            .expect("project list")
            .iter()
            .any(|listed| listed.id == project.id),
        "a removed project leaves the list"
    );
    assert!(project_path.is_dir(), "the project folder stays on disk");

    let readded = journal
        .project_add(
            crate::workspace::project_record(project_path.to_str().expect("utf-8 path"))
                .expect("project record"),
        )
        .expect("re-add the same folder");
    assert_eq!(readded.id, project.id, "the same folder keeps its identity");
    assert!(journal
        .projects_list()
        .expect("project list")
        .iter()
        .any(|listed| listed.id == project.id));
    assert!(
        listed_workspace_ids(&journal, &project.id).contains(&local.id),
        "its rows come back with it"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn removing_an_unknown_project_is_refused() {
    let (dir, registry, journal) = tmp_delete_registry();

    assert!(
        registry.project_remove("p.missing").is_err(),
        "an unknown project is refused, never reported as removed"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_removed_project_takes_no_new_row_and_lists_none() {
    let (dir, registry, journal) = tmp_delete_registry();
    let (_, project, _local) = project_with_local_row(&dir, &journal);
    registry.project_remove(&project.id).expect("remove");

    assert!(
        journal
            .workspace_create(crate::workspace::local_workspace_record(&project))
            .is_err(),
        "a row cannot be added under a removed project"
    );
    assert!(
        registry
            .workspace_create(&project.id, WorkspaceIsolation::Local, None)
            .is_err(),
        "a create through the registry is refused too"
    );
    assert!(
        journal.workspaces_list(&project.id).is_err(),
        "a removed project lists no workspaces"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_create_refuses_while_its_project_is_being_removed_and_again_once_it_is_listed() {
    let (dir, registry, journal) = tmp_delete_registry();
    let (_, project, _local) = project_with_local_row(&dir, &journal);

    let removal = registry
        .mark_project_removing(&project.id)
        .expect("the mark goes up");
    assert!(
        registry
            .workspace_create(&project.id, WorkspaceIsolation::Local, None)
            .is_err(),
        "nothing is created while the project leaves the list"
    );
    assert!(
        registry.mark_project_removing(&project.id).is_err(),
        "a second removal of the same project is refused"
    );
    drop(removal);

    assert!(
        registry
            .workspace_create(&project.id, WorkspaceIsolation::Local, None)
            .is_ok(),
        "the mark clears when its removal settles"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_session_cannot_start_in_a_row_of_a_removed_project() {
    let (dir, registry, journal) = tmp_delete_registry();
    let (_, project, local) = project_with_local_row(&dir, &journal);
    assert!(registry.workspace_creation_guard(Some(&local.id)).is_ok());
    registry.project_remove(&project.id).expect("remove");

    assert!(
        registry.workspace_creation_guard(Some(&local.id)).is_err(),
        "a row of a removed project takes no session"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
