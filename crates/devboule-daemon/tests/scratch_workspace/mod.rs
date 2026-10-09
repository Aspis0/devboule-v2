//! A workspace for a fixture session to start in. The wire door refuses a
//! session that names none, so every fixture asks for a scratch project first.

use devboule_daemon::test_dirs::test_temp_dir;
use devboule_daemon::DaemonClient;
use devboule_protocol::WorkspaceIsolation;

pub fn scratch_workspace(client: &DaemonClient) -> String {
    let dir = test_temp_dir("scratch workspace");
    std::fs::create_dir_all(&dir).expect("scratch project dir");
    let project = client
        .project_add(&dir.to_string_lossy())
        .expect("scratch project add");
    client
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("scratch workspace create")
        .id
}
