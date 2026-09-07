//! The commands' behaviour as the panel sees it: actionable errors when no
//! workspace is chosen, workspace path validation, persistence, and the
//! environment override winning over saved settings.

use std::fs;
use std::path::PathBuf;

use devboule_protocol::ErrorCode;

use super::support::{
    assert_actionable, runtime_with_config, TestEnvironment, UnreadableDirectory,
};
use crate::backend::error::CommandError;
use crate::oracle::commands::{
    oracle_ask_inner, oracle_doctor_inner, oracle_files_inner, oracle_index_cancel_inner,
    oracle_index_start_inner, oracle_model_download_cancel_inner,
    oracle_model_download_start_inner, oracle_stats_inner, oracle_status_inner,
    oracle_workspace_get_inner, oracle_workspace_set_inner,
};
use crate::oracle::runtime::{PersistedOracleSettings, ORACLE_ROOT_ENV, ORACLE_SETTINGS_FILE};
use crate::oracle::{oracle_watch_start, oracle_watch_stop, FileTab, OracleRuntime};

#[test]
fn commands_explain_that_a_workspace_must_be_chosen() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = runtime_with_config(&temp.path().join("config"));

    let workspace = oracle_workspace_get_inner(&runtime).expect("workspace getter");
    assert!(workspace.path.is_none());
    assert_eq!(workspace.source, "unset");

    let model_error = oracle_model_download_start_inner(&runtime).expect_err("no workspace");
    assert_actionable(model_error, &["no workspace", "choose"]);

    let status_error =
        tauri::async_runtime::block_on(oracle_status_inner(&runtime)).expect_err("no workspace");
    assert_actionable(status_error, &["no workspace", "choose"]);

    let stats_error =
        tauri::async_runtime::block_on(oracle_stats_inner(&runtime)).expect_err("no workspace");
    assert_actionable(stats_error, &["no workspace", "choose"]);

    let files_error =
        tauri::async_runtime::block_on(oracle_files_inner(&runtime, FileTab::Indexed, 1))
            .expect_err("no workspace");
    assert_actionable(files_error, &["no workspace", "choose"]);

    let ask_error = tauri::async_runtime::block_on(oracle_ask_inner(
        &runtime,
        "find the deployment code".to_string(),
    ))
    .expect_err("no workspace");
    assert_actionable(ask_error, &["no workspace", "choose"]);

    let index_error = oracle_index_start_inner(&runtime).expect_err("no workspace");
    assert_actionable(index_error, &["no workspace", "choose"]);

    let doctor = tauri::async_runtime::block_on(oracle_doctor_inner(&runtime))
        .expect("doctor returns a health explanation");
    assert_eq!(doctor.state, "unavailable");
    assert_eq!(doctor.checks[0].id, "configuration");
    assert_eq!(doctor.checks[0].state, "failed");
    assert_actionable(
        CommandError::new(
            ErrorCode::InvalidRequest,
            doctor.checks[0]
                .message
                .clone()
                .expect("configuration check message"),
        ),
        &["no workspace", "choose"],
    );

    assert_actionable(
        oracle_watch_start().expect_err("watcher is not implemented"),
        &["watching", "not implemented"],
    );
    assert_actionable(
        oracle_watch_stop().expect_err("watcher is not implemented"),
        &["watching", "not implemented"],
    );

    // Cancellation is intentionally idempotent because the panel invokes it
    // during cleanup, including when the first-run workspace is still unset.
    oracle_model_download_cancel_inner(&runtime).expect("cancel is safe before start");
    oracle_index_cancel_inner(&runtime).expect("cancel is safe before start");
}

#[test]
fn workspace_path_errors_tell_the_user_how_to_fix_them() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = runtime_with_config(&temp.path().join("config"));

    let relative = oracle_workspace_set_inner(&runtime, "relative/oracle")
        .expect_err("relative path must be rejected");
    assert_actionable(relative, &["absolute", "relative"]);

    let missing = temp.path().join("does-not-exist");
    let missing_error = oracle_workspace_set_inner(&runtime, missing.to_str().unwrap())
        .expect_err("missing folder must be rejected");
    assert_actionable(missing_error, &["workspace", "exists", "choose"]);

    let file = temp.path().join("not-a-folder.txt");
    fs::write(&file, "not a directory").expect("file");
    let file_error = oracle_workspace_set_inner(&runtime, file.to_str().unwrap())
        .expect_err("file must be rejected as a workspace");
    assert_actionable(file_error, &["not a folder", "choose"]);
}

#[test]
fn unreadable_workspace_path_has_an_actionable_error() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = runtime_with_config(&temp.path().join("config"));
    let unreadable = temp.path().join("unreadable");
    fs::create_dir(&unreadable).expect("unreadable directory");

    let _permissions = UnreadableDirectory::new(&unreadable);
    let error = oracle_workspace_set_inner(&runtime, unreadable.to_str().unwrap())
        .expect_err("unreadable folder must be rejected");
    assert_actionable(error, &["workspace", "read", "permissions", "choose"]);
}

#[test]
fn persisted_workspace_round_trips_and_corruption_is_reported() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let config = temp.path().join("config");
    let selected = temp.path().join("selected");
    fs::create_dir(&selected).expect("selected directory");

    let runtime = runtime_with_config(&config);
    let selected_workspace =
        oracle_workspace_set_inner(&runtime, selected.to_str().unwrap()).expect("choose workspace");
    assert_eq!(selected_workspace.source, "saved");
    let selected_path = PathBuf::from(
        selected_workspace
            .path
            .as_deref()
            .expect("selected workspace path"),
    );
    assert!(selected_path.is_absolute());
    assert_eq!(selected_path.file_name(), selected.file_name());

    let settings_path = config.join(ORACLE_SETTINGS_FILE);
    let settings: PersistedOracleSettings =
        serde_json::from_str(&fs::read_to_string(&settings_path).expect("settings file"))
            .expect("persisted settings JSON");
    assert_eq!(settings.oracle_root, selected_path.to_string_lossy());

    let reloaded = runtime_with_config(&config);
    let reloaded_workspace = oracle_workspace_get_inner(&reloaded).expect("workspace getter");
    assert_eq!(reloaded_workspace.source, "saved");
    assert_eq!(reloaded_workspace.path, selected_workspace.path);

    fs::write(&settings_path, r#"{"oracle_root":"#).expect("truncated settings");
    let corrupted = OracleRuntime::from_environment();
    let error = corrupted
        .load_persisted_root(&config)
        .expect_err("truncated settings must not be swallowed");
    assert_actionable(error, &["preferences", "invalid json", "choose"]);
    assert!(
        corrupted.workspace().path.is_none(),
        "invalid settings must not select a partial workspace"
    );
}

#[test]
fn environment_workspace_wins_for_multiple_commands() {
    let env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let config = temp.path().join("config");
    let saved = temp.path().join("saved");
    let environment = temp.path().join("environment");
    fs::create_dir(&saved).expect("saved directory");
    fs::create_dir(&environment).expect("environment directory");
    fs::write(environment.join("environment.txt"), "environment sentinel")
        .expect("environment file");
    fs::create_dir_all(&config).expect("config directory");
    fs::write(
        config.join(ORACLE_SETTINGS_FILE),
        serde_json::to_vec(&PersistedOracleSettings {
            oracle_root: saved.to_str().unwrap().to_string(),
        })
        .expect("settings JSON"),
    )
    .expect("saved settings");
    env.set(ORACLE_ROOT_ENV, environment.to_str().unwrap());

    let runtime = OracleRuntime::from_environment();
    runtime
        .load_persisted_root(&config)
        .expect("environment override bypasses saved settings");

    let workspace = oracle_workspace_get_inner(&runtime).expect("workspace getter");
    assert_eq!(workspace.source, "environment");
    assert!(!workspace.editable);
    assert_eq!(
        workspace.path,
        Some(environment.to_str().unwrap().to_string())
    );

    let files = tauri::async_runtime::block_on(oracle_files_inner(&runtime, FileTab::Pending, 1))
        .expect("files command");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "environment.txt");

    let stats =
        tauri::async_runtime::block_on(oracle_stats_inner(&runtime)).expect("stats command");
    assert_eq!(stats.pending_files, 1);
    assert_eq!(stats.indexed_files, 0);

    let status =
        tauri::async_runtime::block_on(oracle_status_inner(&runtime)).expect("status command");
    assert_eq!(status.total_files, 1);
    assert_eq!(status.pending_files, 1);
    assert_eq!(status.indexed_files, 0);
}
