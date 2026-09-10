//! Reading the index snapshot and turning it into the status and health
//! answers the panel shows, including the model readiness checks that gate
//! indexing and queries.

use std::sync::atomic::Ordering;

use oracle_core::configured_model_present;
use oracle_core::{BackendChoice, IndexStatusSnapshot, LanceStore, SqliteStore};

use crate::backend::error::CommandError;

use super::errors::{core_error, invalid_configuration};
use super::runtime::OracleRuntime;
use super::runtime::ResolvedOraclePaths;
use super::types::{
    OracleFolderIndexState, OracleHealthCheck, OracleIndexStatus, OracleModelState,
    OracleModelStatus, OracleResourceBudget,
};

pub(super) async fn read_index_snapshot(
    paths: &ResolvedOraclePaths,
) -> Result<IndexStatusSnapshot, CommandError> {
    let sqlite = SqliteStore::new(&paths.data.metadata)
        .map_err(|error| core_error("opening Oracle metadata store failed", error))?;
    let chunk_vectors = LanceStore::new(&paths.data.chunks);
    oracle_core::chunk_index_status(
        &paths.workspace,
        &sqlite,
        &chunk_vectors,
        &paths.data.manifest,
    )
    .await
    .map_err(|error| core_error("reading Oracle index status failed", error))
}

pub(super) fn status_from_snapshot(
    runtime: &OracleRuntime,
    snapshot: &IndexStatusSnapshot,
) -> OracleIndexStatus {
    let state = if runtime.indexing.load(Ordering::Acquire) {
        "indexing"
    } else if runtime.index_error().is_some() {
        "error"
    } else if snapshot.pending_files > 0 {
        "incomplete"
    } else if snapshot.stale_files > 0 {
        "stale"
    } else if snapshot.indexed_files == 0 {
        // Nothing indexed yet, whether or not files are expected. The contract
        // has no separate "empty" state, so both cases report idle.
        "idle"
    } else {
        "ready"
    };
    OracleIndexStatus {
        state: state.to_string(),
        indexed_files: snapshot.indexed_files,
        total_files: snapshot.expected_files,
        indexed_chunks: snapshot.sqlite_chunks,
        pending_files: snapshot.pending_files,
        stale_files: snapshot.stale_files,
        resource_budget: OracleResourceBudget {
            max_cpu_percent: 20.0,
            max_memory_mb: 768.0,
            max_parallelism: 1.0,
        },
        model: runtime.model_status(),
        reranker: Some(runtime.reranker_status()),
        pause_reason: snapshot.pause_reason.clone(),
    }
}

/// Fold an index snapshot into the answer for a folder that this runtime is
/// not indexing.
///
/// The ordering and thresholds mirror [`status_from_snapshot`] — pending
/// first, then stale, then "nothing indexed", then ready — but the
/// runtime-only states (`indexing`, `error`) do not apply to a folder probed
/// on request, so they are not produced here. The one deliberate difference:
/// a snapshot with zero indexed files is `never_indexed` even when files are
/// pending, because the question is "does this folder have an index", and
/// nothing in the index is the honest answer. Chunks counted with no matching
/// manifest entry are `partial` instead: the data is there but cannot be
/// mapped back to files, which is not the same as an empty index.
pub(super) fn folder_state_from_snapshot(snapshot: &IndexStatusSnapshot) -> OracleFolderIndexState {
    if snapshot.indexed_files == 0 {
        if snapshot.sqlite_chunks > 0 {
            OracleFolderIndexState::Partial
        } else {
            OracleFolderIndexState::NeverIndexed
        }
    } else if snapshot.pending_files > 0 || snapshot.stale_files > 0 {
        OracleFolderIndexState::Partial
    } else {
        OracleFolderIndexState::Ready
    }
}

pub(super) fn model_health_check(
    backend: &BackendChoice,
    model_status: &OracleModelStatus,
) -> OracleHealthCheck {
    match backend {
        BackendChoice::Ort { model_dir, int8 } => {
            if configured_model_present(model_dir, *int8) {
                let message = format!(
                    "Model `{}` is ready at {}.",
                    model_status.model_id,
                    model_dir.display()
                );
                health_check("embedder", "ok", Some(&message))
            } else {
                let message = if model_status.state == OracleModelState::Downloading {
                    format!(
                        "Downloading `{}` (about {} MB) to {}{}.",
                        model_status.model_id,
                        model_status.approximate_bytes / 1_000_000,
                        model_dir.display(),
                        model_status
                            .file
                            .as_deref()
                            .map(|file| format!("; current file {file}"))
                            .unwrap_or_default()
                    )
                } else {
                    model_status.message.clone().unwrap_or_else(|| {
                        format!(
                            "Model `{}` is not ready. Oracle looks in {} and needs about {} MB.",
                            model_status.model_id,
                            model_dir.display(),
                            model_status.approximate_bytes / 1_000_000
                        )
                    })
                };
                health_check("embedder", "failed", Some(&message))
            }
        }
        BackendChoice::Candle { .. } => health_check(
            "embedder",
            "unknown",
            Some("Candle checks its model cache when the model is first loaded."),
        ),
    }
}

pub(super) fn ensure_model_is_available(
    backend: &BackendChoice,
    model_status: &OracleModelStatus,
) -> Result<(), CommandError> {
    if let BackendChoice::Ort { model_dir, int8 } = backend {
        let config_path = model_dir.join("model_config.json");
        if !config_path.is_file() {
            return Err(invalid_configuration(
                format!(
                    "Oracle model `{}` is not ready: {} is missing model_config.json. The model download is about {} MB; wait for it to finish or retry it in the Oracle panel.",
                    model_status.model_id,
                    model_dir.display(),
                    model_status.approximate_bytes / 1_000_000
                ),
            ));
        }
        if !configured_model_present(model_dir, *int8) {
            return Err(invalid_configuration(format!(
                "Oracle model `{}` is not ready: its ONNX graph or tokenizer is missing under {}. Wait for the download to finish, or retry it in the Oracle panel.",
                model_status.model_id,
                model_dir.display(),
            )));
        }
    }
    Ok(())
}

pub(super) fn backend_label(backend: &BackendChoice) -> String {
    match backend {
        BackendChoice::Candle { .. } => "candle".to_string(),
        BackendChoice::Ort { int8, .. } => {
            if *int8 {
                "onnx-int8".to_string()
            } else {
                "onnx-fp32".to_string()
            }
        }
    }
}

pub(super) fn health_check(id: &str, state: &str, message: Option<&str>) -> OracleHealthCheck {
    OracleHealthCheck {
        id: id.to_string(),
        state: state.to_string(),
        message: message.map(str::to_string),
    }
}
