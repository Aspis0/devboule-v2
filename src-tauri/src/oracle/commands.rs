//! The Tauri command surface for Oracle: one wrapper per registered command,
//! the inner functions that carry the actual behaviour, and the index job that
//! runs on its own thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::State;

use devboule_protocol::ErrorCode;
use oracle_core::{
    collect_text_files, file_needs_index, index_file_chunks, load_manifest,
    manifest_files_for_root, prune_excluded_chunks, CancelFlag, IndexerConfig, LanceStore,
    SqliteStore, TextEmbedder,
};

use crate::backend::error::CommandError;

use super::errors::{core_error, unimplemented_command};
use super::query::{open_engine, search_paths, validate_query};
use super::runtime::OracleRuntime;
use super::runtime::ResolvedOraclePaths;
use super::status::{
    backend_label, ensure_model_is_available, health_check, model_health_check,
    read_index_snapshot, status_from_snapshot,
};
use super::types::{
    FileTab, IndexedFile, OracleHealth, OracleIndexStats, OracleIndexStatus, OracleSearchResponse,
    OracleWorkspace,
};

const PAGE_SIZE: usize = 50;

pub(super) fn oracle_workspace_get_inner(
    runtime: &OracleRuntime,
) -> Result<OracleWorkspace, CommandError> {
    Ok(runtime.workspace())
}

pub(super) fn oracle_workspace_set_inner(
    runtime: &OracleRuntime,
    path: &str,
) -> Result<OracleWorkspace, CommandError> {
    runtime.set_workspace(path)
}

pub(super) fn oracle_model_download_start_inner(
    runtime: &OracleRuntime,
) -> Result<(), CommandError> {
    runtime.start_model_download(true)
}

pub(super) fn oracle_model_download_cancel_inner(
    runtime: &OracleRuntime,
) -> Result<(), CommandError> {
    runtime.cancel_model_download();
    Ok(())
}

pub(super) fn oracle_index_cancel_inner(runtime: &OracleRuntime) -> Result<(), CommandError> {
    runtime.cancel_index();
    Ok(())
}

pub(super) async fn oracle_status_inner(
    runtime: &OracleRuntime,
) -> Result<OracleIndexStatus, CommandError> {
    let paths = runtime.paths()?;
    runtime.start_model_download(false)?;
    let snapshot = read_index_snapshot(&paths).await?;
    Ok(status_from_snapshot(runtime, &snapshot))
}

pub(super) async fn oracle_doctor_inner(
    runtime: &OracleRuntime,
) -> Result<OracleHealth, CommandError> {
    let paths = match runtime.paths() {
        Ok(paths) => paths,
        Err(error) => {
            return Ok(OracleHealth {
                state: "unavailable".to_string(),
                checks: vec![health_check(
                    "configuration",
                    "failed",
                    Some(error.message.as_str()),
                )],
                message: Some(
                    "Choose an existing workspace folder in the Oracle panel. Developers can alternatively set DEVBOULE_ORACLE_ROOT to an absolute path.".to_string(),
                ),
            });
        }
    };

    runtime.start_model_download(false)?;

    let mut checks = Vec::new();
    checks.push(health_check("workspace", "ok", None));

    let stores_ok = SqliteStore::new(&paths.data.metadata).is_ok() && paths.data.chunks.exists();
    checks.push(if stores_ok {
        health_check(
            "stores",
            "ok",
            Some("SQLite and chunk vectors are available."),
        )
    } else {
        health_check(
            "stores",
            "failed",
            Some("SQLite or the chunk vector store is unavailable."),
        )
    });

    let index_check = match read_index_snapshot(&paths).await {
        Ok(snapshot) => {
            if snapshot.pending_files == 0 && snapshot.stale_files == 0 {
                health_check("index", "ok", Some("The workspace index is current."))
            } else {
                health_check(
                    "index",
                    "failed",
                    Some("The workspace index has pending or stale files."),
                )
            }
        }
        Err(_) => health_check(
            "index",
            "failed",
            Some("The workspace index cannot be read."),
        ),
    };
    checks.push(index_check);

    let pool = runtime.pool()?;
    let model_status = runtime.model_status();
    checks.push(model_health_check(pool.backend(), &model_status));

    let query_check = match open_engine(&paths, None) {
        Ok(engine) => match engine.health().await {
            Ok(_) => health_check("query", "ok", Some("Oracle query stores are readable.")),
            Err(_) => health_check(
                "query",
                "failed",
                Some("Oracle query stores are unreadable."),
            ),
        },
        Err(_) => health_check(
            "query",
            "failed",
            Some("Oracle query stores are unavailable."),
        ),
    };
    checks.push(query_check);

    checks.push(health_check(
        "watcher",
        "failed",
        Some("Filesystem watching is not implemented in M4."),
    ));

    let all_ok = checks.iter().all(|check| check.state == "ok");
    Ok(OracleHealth {
        state: if all_ok { "healthy" } else { "degraded" }.to_string(),
        checks,
        message: if all_ok {
            None
        } else {
            Some("Oracle needs attention before it can be considered healthy.".to_string())
        },
    })
}

pub(super) async fn oracle_stats_inner(
    runtime: &OracleRuntime,
) -> Result<OracleIndexStats, CommandError> {
    let paths = runtime.paths()?;
    let snapshot = read_index_snapshot(&paths).await?;
    let pool = runtime.pool()?;
    Ok(OracleIndexStats {
        indexed_files: snapshot.indexed_files,
        indexed_chunks: snapshot.sqlite_chunks,
        pending_files: snapshot.pending_files,
        stale_files: snapshot.stale_files,
        backend: backend_label(pool.backend()),
    })
}

pub(super) async fn oracle_files_inner(
    runtime: &OracleRuntime,
    tab: FileTab,
    page: usize,
) -> Result<Vec<IndexedFile>, CommandError> {
    let paths = runtime.paths()?;
    let sqlite = SqliteStore::new(&paths.data.metadata)
        .map_err(|error| core_error("opening Oracle metadata store failed", error))?;
    let mut manifest = load_manifest(&paths.data.manifest);
    let entries = manifest_files_for_root(&mut manifest, &paths.workspace, false)
        .cloned()
        .unwrap_or_default();

    let mut files = Vec::new();
    for path in collect_text_files(&paths.workspace) {
        let file_id = match path.strip_prefix(&paths.workspace) {
            Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        let entry = entries.get(&file_id);
        let is_pending = entry.is_none();
        let is_stale = match entry {
            Some(_) => file_needs_index(&path, &paths.workspace, &entries, &sqlite)
                .map_err(|error| core_error("checking Oracle file freshness failed", error))?,
            None => false,
        };
        let include = match tab {
            FileTab::Indexed => !is_pending && !is_stale,
            FileTab::Pending => is_pending,
            FileTab::Stale => is_stale,
        };
        if !include {
            continue;
        }

        let chunks = entry
            .and_then(|record| record.chunks)
            .map(|count| count as usize)
            .unwrap_or(0);
        let updated_at = entry
            .map(|record| record.updated_at.clone())
            .unwrap_or_else(|| "not indexed".to_string());
        files.push(IndexedFile {
            path: file_id,
            chunks,
            updated_at,
        });
    }

    files.sort_by(|left, right| left.path.cmp(&right.path));
    let offset = page.saturating_sub(1).saturating_mul(PAGE_SIZE);
    Ok(files.into_iter().skip(offset).take(PAGE_SIZE).collect())
}

pub(super) async fn oracle_ask_inner(
    runtime: &OracleRuntime,
    query: String,
) -> Result<OracleSearchResponse, CommandError> {
    let query = validate_query(query)?;
    let paths = runtime.paths()?;
    let pool = runtime.pool()?;
    runtime.start_model_download(false)?;
    let model_status = runtime.model_status();
    search_paths(&paths, &query, &pool, runtime.reranker(), &model_status).await
}

#[tauri::command]
pub fn oracle_workspace_get(
    runtime: State<'_, OracleRuntime>,
) -> Result<OracleWorkspace, CommandError> {
    oracle_workspace_get_inner(&runtime)
}

#[tauri::command]
pub fn oracle_workspace_set(
    runtime: State<'_, OracleRuntime>,
    path: String,
) -> Result<OracleWorkspace, CommandError> {
    oracle_workspace_set_inner(&runtime, &path)
}

#[tauri::command]
pub fn oracle_model_download_start(runtime: State<'_, OracleRuntime>) -> Result<(), CommandError> {
    oracle_model_download_start_inner(&runtime)
}

#[tauri::command]
pub fn oracle_model_download_cancel(runtime: State<'_, OracleRuntime>) -> Result<(), CommandError> {
    oracle_model_download_cancel_inner(&runtime)
}

#[tauri::command]
pub fn oracle_index_cancel(runtime: State<'_, OracleRuntime>) -> Result<(), CommandError> {
    oracle_index_cancel_inner(&runtime)
}

#[tauri::command]
pub async fn oracle_status(
    runtime: State<'_, OracleRuntime>,
) -> Result<OracleIndexStatus, CommandError> {
    oracle_status_inner(&runtime).await
}

#[tauri::command]
pub async fn oracle_doctor(
    runtime: State<'_, OracleRuntime>,
) -> Result<OracleHealth, CommandError> {
    oracle_doctor_inner(&runtime).await
}

#[tauri::command]
pub async fn oracle_stats(
    runtime: State<'_, OracleRuntime>,
) -> Result<OracleIndexStats, CommandError> {
    oracle_stats_inner(&runtime).await
}

pub(super) fn oracle_index_start_inner(runtime: &OracleRuntime) -> Result<(), CommandError> {
    let paths = runtime.paths()?;
    let pool = runtime.pool()?;
    runtime.start_model_download(false)?;
    ensure_model_is_available(pool.backend(), &runtime.model_status())?;
    let embedder: Arc<dyn TextEmbedder> = pool;

    if runtime.indexing.swap(true, Ordering::AcqRel) {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            "Oracle indexing is already running.",
        ));
    }
    *runtime
        .last_index_error
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;

    let indexing = Arc::clone(&runtime.indexing);
    let index_cancel = Arc::clone(&runtime.index_cancel);
    let last_index_error = Arc::clone(&runtime.last_index_error);
    let cancel = CancelFlag::new();
    *runtime
        .index_cancel
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(cancel.clone());
    let thread_result = std::thread::Builder::new()
        .name("oracle-index".to_string())
        .spawn(move || {
            run_index_job(
                paths,
                embedder,
                indexing,
                index_cancel,
                last_index_error,
                cancel,
            );
        });

    if let Err(error) = thread_result {
        runtime.indexing.store(false, Ordering::Release);
        runtime
            .index_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        *runtime
            .last_index_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(format!("starting Oracle indexing failed: {error}"));
        return Err(CommandError::new(
            ErrorCode::Io,
            "Could not start the Oracle indexing task.",
        ));
    }

    Ok(())
}

#[tauri::command]
pub fn oracle_index_start(runtime: State<'_, OracleRuntime>) -> Result<(), CommandError> {
    oracle_index_start_inner(&runtime)
}

pub(super) fn run_index_job(
    paths: ResolvedOraclePaths,
    embedder: Arc<dyn TextEmbedder>,
    indexing: Arc<AtomicBool>,
    index_cancel: Arc<Mutex<Option<CancelFlag>>>,
    last_index_error: Arc<Mutex<Option<String>>>,
    cancel: CancelFlag,
) {
    let sqlite = match SqliteStore::new(&paths.data.metadata) {
        Ok(sqlite) => sqlite,
        Err(error) => {
            finish_index(
                &indexing,
                &index_cancel,
                &last_index_error,
                &cancel,
                Some(format!("opening Oracle metadata store failed: {error}")),
            );
            return;
        }
    };
    let chunk_vectors = LanceStore::new(&paths.data.chunks);
    // Build the code-knowledge graph alongside the index. Without this the
    // store exists, is exported, and stays empty — which is what it did until
    // now, and what made "the CKG only needs its read queries" a false premise.
    let config = IndexerConfig {
        ckg_path: Some(paths.data.ckg.clone()),
        ..IndexerConfig::default()
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tauri::async_runtime::block_on(index_file_chunks(
            &paths.workspace,
            &sqlite,
            &chunk_vectors,
            &paths.data.manifest,
            embedder.as_ref(),
            &cancel,
            &config,
            None,
        ))
    }));
    // Deleting a file from the workspace used to leave its chunks, its vectors
    // and its manifest entry in place for ever, so Oracle went on citing a file
    // that was not there. `prune_excluded_chunks` had existed since the port and
    // was reachable only from its own tests — a real function with no caller,
    // which is indistinguishable from a missing one until someone deletes a
    // file. It runs here, after a successful run, on the same stores.
    //
    // Not on a cancelled run — but not for the reason first written here. That
    // comment said a partial index "would delete the files it never reached",
    // which is false: the prune removes files that have chunks in the index and
    // no file on disk, and a file the run never reached has no chunks, so it
    // cannot be removed. Completeness of the run is irrelevant to correctness.
    //
    // The real reason is narrower: cancelling is the user asking us to stop
    // doing work, and a prune is work. Note that a run PAUSED for memory or a
    // batch limit still returns `Ok(Ok(_))` and does prune, which is both
    // intended and safe by the argument above.
    if matches!(result, Ok(Ok(_))) && !cancel.is_cancelled() {
        let node_vectors = LanceStore::new(&paths.data.vectors);
        match tauri::async_runtime::block_on(prune_excluded_chunks(
            &paths.workspace,
            &sqlite,
            &chunk_vectors,
            &paths.data.manifest,
            Some(&node_vectors),
            Some(&paths.data.ckg),
            None,
        )) {
            Ok(pruned) if pruned.removed_files > 0 => {
                eprintln!(
                    "[oracle] pruned {} file(s) that no longer exist, {} vector(s)",
                    pruned.removed_files, pruned.removed_vectors
                );
            }
            Ok(_) => {}
            // A prune failure leaves stale rows, which is worse than nothing but
            // far better than losing an index that just finished building.
            Err(error) => eprintln!("[oracle] pruning deleted files failed: {error:#}"),
        }
    }

    match result {
        Ok(Ok(_)) => finish_index(&indexing, &index_cancel, &last_index_error, &cancel, None),
        Ok(Err(error)) => finish_index(
            &indexing,
            &index_cancel,
            &last_index_error,
            &cancel,
            Some(format!("Oracle indexing failed: {error}")),
        ),
        Err(_) => finish_index(
            &indexing,
            &index_cancel,
            &last_index_error,
            &cancel,
            Some("Oracle indexing task panicked.".to_string()),
        ),
    }
}

#[tauri::command]
pub fn oracle_watch_start() -> Result<(), CommandError> {
    Err(unimplemented_command(
        "Oracle filesystem watching is not implemented in M4.",
    ))
}

#[tauri::command]
pub fn oracle_watch_stop() -> Result<(), CommandError> {
    Err(unimplemented_command(
        "Oracle filesystem watching is not implemented in M4.",
    ))
}

#[tauri::command]
pub async fn oracle_files(
    runtime: State<'_, OracleRuntime>,
    tab: FileTab,
    page: usize,
) -> Result<Vec<IndexedFile>, CommandError> {
    oracle_files_inner(&runtime, tab, page).await
}

#[tauri::command]
pub async fn oracle_ask(
    runtime: State<'_, OracleRuntime>,
    query: String,
) -> Result<OracleSearchResponse, CommandError> {
    oracle_ask_inner(&runtime, query).await
}

fn finish_index(
    indexing: &AtomicBool,
    index_cancel: &Mutex<Option<CancelFlag>>,
    last_index_error: &Mutex<Option<String>>,
    cancel: &CancelFlag,
    error: Option<String>,
) {
    let error = if cancel.is_cancelled() { None } else { error };
    *last_index_error
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = error;
    index_cancel
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    indexing.store(false, Ordering::Release);
}
