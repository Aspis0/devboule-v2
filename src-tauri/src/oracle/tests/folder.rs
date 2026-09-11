//! The folder-scoped commands: what `oracle_folder_status` answers for a
//! never-indexed, a fully indexed, a partial, and an unreadable folder, and
//! that neither folder command moves the runtime's active root.

use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use oracle_core::{
    index_file_chunks, CancelFlag, IndexerConfig, LanceStore, OracleDataPaths, SqliteStore,
};

use super::support::{assert_actionable, SlowTestEmbedder, TestEnvironment, UnreadableDirectory};
use crate::oracle::commands::oracle_workspace_get_inner;
use crate::oracle::folder::{
    ensure_folder_index_is_usable, oracle_ask_folder_inner, oracle_folder_status_inner,
    probe_folder, resolve_folder_root,
};
use crate::oracle::runtime::OracleRuntime;
use crate::oracle::OracleFolderIndexState;

/// Build a real index for `root` with the fake embedder the other Oracle
/// command tests use. This is the same pipeline the app runs, minus the model.
fn index_folder(root: &Path) {
    let paths = OracleDataPaths::from_root(root);
    let sqlite = SqliteStore::new(&paths.metadata).expect("metadata store");
    let chunk_vectors = LanceStore::new(&paths.chunks);
    let embedder = SlowTestEmbedder {
        started: Arc::new(AtomicBool::new(false)),
    };
    let cancel = CancelFlag::new();
    let config = IndexerConfig {
        // A predictable, non-waiting run: the tests are not measuring memory
        // pressure and must not block on the machine's free RAM.
        min_free_gb: 0.0,
        max_gpu_temp_c: None,
        ckg_path: None,
        ..IndexerConfig::default()
    };
    tauri::async_runtime::block_on(index_file_chunks(
        root,
        &sqlite,
        &chunk_vectors,
        &paths.manifest,
        &embedder,
        &cancel,
        &config,
        None,
    ))
    .expect("indexing the test folder");
}

fn canonical(path: &Path) -> std::path::PathBuf {
    fs::canonicalize(path).expect("canonicalize test folder")
}

#[test]
fn a_folder_that_was_never_indexed_reports_never_indexed_without_writing() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let folder = temp.path().join("untouched");
    fs::create_dir(&folder).expect("folder");
    fs::write(folder.join("notes.txt"), "a file nobody indexed\n").expect("file");

    let status =
        tauri::async_runtime::block_on(oracle_folder_status_inner(folder.to_str().unwrap()))
            .expect("a folder with no index is an answer, not an error");

    assert_eq!(status.state, OracleFolderIndexState::NeverIndexed);
    assert_eq!(status.indexed_files, 0);
    assert_eq!(status.pending_files, 0);
    assert!(status.message.is_some(), "the answer explains itself");

    // The probe is read-only: asking the question must not bring the data
    // directory (or a store inside it) into existence.
    assert!(
        !folder.join("oracle-data").exists(),
        "probing a folder must not create its Oracle data directory"
    );
}

#[test]
fn a_fully_indexed_folder_reports_ready() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let folder = temp.path().join("indexed");
    fs::create_dir(&folder).expect("folder");
    fs::write(
        folder.join("notes.txt"),
        "Oracle keeps an index of this folder. The status command must see it.\n\
         A second sentence gives the chunker enough text to emit a chunk.\n",
    )
    .expect("file");

    let root = canonical(&folder);
    index_folder(&root);

    let status = tauri::async_runtime::block_on(oracle_folder_status_inner(root.to_str().unwrap()))
        .expect("status of an indexed folder");

    assert_eq!(status.state, OracleFolderIndexState::Ready);
    assert_eq!(status.indexed_files, 1);
    assert_eq!(status.total_files, 1);
    assert_eq!(status.pending_files, 0);
    assert_eq!(status.stale_files, 0);
    assert!(
        status.indexed_chunks > 0,
        "a ready index must have counted its chunks"
    );
    assert_eq!(status.message, None);
}

#[test]
fn a_folder_with_pending_files_reports_partial() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let folder = temp.path().join("partial");
    fs::create_dir(&folder).expect("folder");
    fs::write(folder.join("indexed.txt"), "this file is indexed\n").expect("file");

    let root = canonical(&folder);
    index_folder(&root);

    // A file added after the run is pending by definition.
    fs::write(
        folder.join("added-later.txt"),
        "this file is not indexed yet\n",
    )
    .expect("file");

    let status = tauri::async_runtime::block_on(oracle_folder_status_inner(root.to_str().unwrap()))
        .expect("status of a partial folder");

    assert_eq!(status.state, OracleFolderIndexState::Partial);
    assert_eq!(status.indexed_files, 1);
    assert_eq!(status.total_files, 2);
    assert_eq!(status.pending_files, 1);
    assert!(status.message.is_some(), "a partial index is explained");
}

#[test]
fn an_unreadable_folder_is_not_reported_as_never_indexed() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let folder = temp.path().join("locked");
    fs::create_dir(&folder).expect("folder");
    fs::write(folder.join("notes.txt"), "indexed before the lock\n").expect("file");

    let root = canonical(&folder);
    index_folder(&root);

    // Readable and complete first, so the only change is readability.
    let before = tauri::async_runtime::block_on(oracle_folder_status_inner(root.to_str().unwrap()))
        .expect("status before the lock");
    assert_eq!(before.state, OracleFolderIndexState::Ready);

    let _permissions = UnreadableDirectory::new(&folder);
    let status =
        tauri::async_runtime::block_on(oracle_folder_status_inner(folder.to_str().unwrap()))
            .expect("an unreadable folder is an answer, not an error");

    assert_eq!(status.state, OracleFolderIndexState::Unreadable);
    assert_ne!(status.state, OracleFolderIndexState::NeverIndexed);
    assert!(
        status.message.is_some(),
        "the unreadable answer explains itself"
    );
}

#[test]
fn a_folder_without_index_artifacts_and_an_unreadable_one_differ() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let empty = temp.path().join("empty");
    let locked = temp.path().join("locked");
    fs::create_dir(&empty).expect("empty folder");
    fs::create_dir(&locked).expect("locked folder");

    let empty_status =
        tauri::async_runtime::block_on(oracle_folder_status_inner(empty.to_str().unwrap()))
            .expect("empty folder status");
    assert_eq!(empty_status.state, OracleFolderIndexState::NeverIndexed);

    let _permissions = UnreadableDirectory::new(&locked);
    let locked_status =
        tauri::async_runtime::block_on(oracle_folder_status_inner(locked.to_str().unwrap()))
            .expect("locked folder status");
    assert_eq!(locked_status.state, OracleFolderIndexState::Unreadable);

    // The whole point of the two states: one says "index it", the other says
    // "look at it first".
    assert_ne!(empty_status.state, locked_status.state);
}

#[test]
fn a_relative_or_missing_folder_is_rejected() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");

    let relative = resolve_folder_root("relative/oracle").expect_err("relative path");
    assert_actionable(relative, &["absolute", "relative"]);

    let missing = temp.path().join("does-not-exist");
    let missing_error = resolve_folder_root(missing.to_str().unwrap()).expect_err("missing folder");
    assert_actionable(missing_error, &["does not exist", "choose"]);

    let file = temp.path().join("not-a-folder.txt");
    fs::write(&file, "a file").expect("file");
    let file_error = resolve_folder_root(file.to_str().unwrap()).expect_err("a file");
    assert_actionable(file_error, &["not a folder", "choose"]);

    let empty = resolve_folder_root("   ").expect_err("empty path");
    assert_actionable(empty, &["empty"]);
}

#[test]
fn querying_a_folder_without_an_index_names_the_folder_and_refuses() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = OracleRuntime::from_environment();
    runtime.configure_root(canonical(temp.path()));

    let folder = temp.path().join("no-index-here");
    fs::create_dir(&folder).expect("folder");

    let error = tauri::async_runtime::block_on(oracle_ask_folder_inner(
        &runtime,
        folder.to_str().unwrap(),
        "anything".to_string(),
    ))
    .expect_err("a folder with no index must not be searched");

    // Read the message before handing the error over: `assert_actionable` takes
    // ownership, so naming the folder has to be checked first.
    assert!(
        error.message.contains(
            folder
                .file_name()
                .expect("folder name")
                .to_string_lossy()
                .as_ref()
        ),
        "the error must name the folder: {}",
        error.message
    );
    assert_actionable(error, &["no oracle index", "index this folder"]);
}

#[test]
fn folder_commands_leave_the_active_root_untouched() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let active = temp.path().join("active");
    let other = temp.path().join("other");
    fs::create_dir(&active).expect("active folder");
    fs::create_dir(&other).expect("other folder");
    fs::write(active.join("active.txt"), "the active workspace\n").expect("file");
    fs::write(other.join("other.txt"), "another folder\n").expect("file");

    let runtime = OracleRuntime::from_environment();
    let active_root = canonical(&active);
    runtime.configure_root(active_root.clone());
    let selected = oracle_workspace_get_inner(&runtime).expect("workspace");
    assert_eq!(
        selected.path.as_deref(),
        Some(active_root.to_string_lossy().as_ref())
    );

    // The status command cannot move the root: it does not receive the
    // runtime at all. Its answer for another folder is still real.
    let status =
        tauri::async_runtime::block_on(oracle_folder_status_inner(other.to_str().unwrap()))
            .expect("status of another folder");
    assert_eq!(status.state, OracleFolderIndexState::NeverIndexed);

    // The query command receives the runtime but must keep its root. With no
    // index in `other` it stops before the model, so this runs deterministically.
    let _ = tauri::async_runtime::block_on(oracle_ask_folder_inner(
        &runtime,
        other.to_str().unwrap(),
        "another folder".to_string(),
    ));

    let after = oracle_workspace_get_inner(&runtime).expect("workspace after folder commands");
    assert_eq!(after.path, selected.path);
    assert_eq!(
        runtime.paths().expect("active paths").workspace,
        active_root,
        "the runtime must still resolve its own workspace"
    );
}

#[test]
fn the_query_gate_accepts_an_indexed_folder_and_describes_the_failure_modes() {
    let _env = TestEnvironment::new("candle");
    let temp = tempfile::tempdir().expect("tempdir");
    let folder = temp.path().join("indexed");
    fs::create_dir(&folder).expect("folder");
    fs::write(
        folder.join("notes.txt"),
        "indexed content for the query gate\n",
    )
    .expect("file");

    let root = canonical(&folder);
    index_folder(&root);
    let probe = probe_folder(&root);
    ensure_folder_index_is_usable(&probe).expect("an indexed folder can be queried");

    // A folder whose metadata store is missing is refused with a message that
    // says what to do, not with a silent empty answer.
    let bare = temp.path().join("bare");
    fs::create_dir(&bare).expect("bare folder");
    let bare_probe = probe_folder(&canonical(&bare));
    let error =
        ensure_folder_index_is_usable(&bare_probe).expect_err("a folder with no index is refused");
    assert_actionable(error, &["no oracle index", "index this folder"]);

    // A corrupt manifest is a different refusal from a missing one.
    let corrupt = temp.path().join("corrupt");
    fs::create_dir_all(corrupt.join("oracle-data")).expect("oracle-data");
    fs::write(
        corrupt.join("oracle-data").join("metadata.sqlite"),
        "not a database",
    )
    .expect("metadata file");
    fs::write(
        corrupt
            .join("oracle-data")
            .join("chunk-index-manifest.json"),
        "{ not json",
    )
    .expect("manifest file");
    let corrupt_probe = probe_folder(&canonical(&corrupt));
    let error =
        ensure_folder_index_is_usable(&corrupt_probe).expect_err("a corrupt manifest is refused");
    assert_actionable(error, &["chunk manifest", "not valid json"]);
}
