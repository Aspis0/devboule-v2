//! The delete road on a Unix box whose external disk or share is gone: the
//! rows name `/Volumes/WorkDisk/...`, the probe answers `NotFound` for that
//! whole subtree while the rest of the filesystem — `/` included — is
//! present, and the delete must refuse and keep the row instead of reading
//! the outage as a deleted folder.

use std::path::Path;

use super::tests::tmp_delete_registry;
use super::*;
use crate::journal::ProjectRecord;
use crate::session::session_workspaces::PresenceProbe;

const DISK: &str = "/Volumes/WorkDisk";

/// A registry holding a project and a worktree workspace recorded on the
/// unplugged disk. `present` is the one path under the disk that still
/// answers, with the metadata of the scratch dir.
fn unplugged_fixture(
    present: Option<&str>,
) -> (std::path::PathBuf, SessionRegistry, Arc<Journal>, String) {
    let (dir, registry, journal) = tmp_delete_registry();
    let project = journal
        .project_add(ProjectRecord {
            id: "p-unplugged".to_string(),
            name: "project".to_string(),
            path: format!("{DISK}/project"),
            git_state: "repository".to_string(),
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .expect("project row");
    let workspace = journal
        .workspace_create(crate::workspace::worktree_workspace_record(
            &project,
            Path::new(&format!("{DISK}/project.worktrees/feature")),
            "feature",
        ))
        .expect("worktree row");
    let present = present.map(str::to_owned);
    let stand_in = dir.clone();
    registry.set_presence_probe_for_test(PresenceProbe {
        metadata: Box::new(move |path: &Path| {
            if present
                .as_deref()
                .is_some_and(|only| path == Path::new(only))
            {
                return std::fs::symlink_metadata(&stand_in);
            }
            if path.starts_with(DISK) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "the disk is not mounted",
                ));
            }
            std::fs::symlink_metadata(path)
        }),
    });
    (dir, registry, journal, workspace.id)
}

fn assert_refused_and_kept(registry: &SessionRegistry, journal: &Journal, workspace_id: &str) {
    let error = registry
        .workspace_delete(workspace_id, false)
        .expect_err("an unplugged volume must refuse, not detach");
    assert_eq!(error.code, ErrorCode::WorkspaceUnavailable);
    assert!(
        error.message.contains("not reachable right now"),
        "the refusal is the pathless volume sentence: {error:?}"
    );
    assert!(
        !error.message.contains('/'),
        "the refusal names no path: {error:?}"
    );
    assert!(
        journal.workspace_get(workspace_id).expect("get").is_some(),
        "the row survives an outage: {error:?}"
    );
}

#[test]
fn a_project_on_an_unplugged_volume_is_refused_and_keeps_its_row() {
    let (dir, registry, journal, workspace_id) = unplugged_fixture(None);
    assert_refused_and_kept(&registry, &journal, &workspace_id);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_checkout_on_an_unplugged_volume_is_refused_and_keeps_its_row() {
    let (dir, registry, journal, workspace_id) =
        unplugged_fixture(Some("/Volumes/WorkDisk/project"));
    assert_refused_and_kept(&registry, &journal, &workspace_id);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
