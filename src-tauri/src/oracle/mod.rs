//! Tauri commands for the local Oracle index and query runtime.
//!
//! The module is split along what the code does:
//!
//! - [`runtime`] owns Oracle's runtime state: the workspace root and how it
//!   was configured, the resolved data paths, the embedder pool and reranker,
//!   and the model download state machine.
//! - [`types`] holds the wire types the commands exchange with the panel.
//! - [`commands`] is the Tauri command surface — one wrapper per registered
//!   command plus the index job that runs on its own thread.
//! - [`folder`] aims the same stores and query engine at a folder that is not
//!   the runtime's active workspace, without switching the runtime's root.
//! - [`status`] reads the index snapshot and turns it into the status and
//!   health answers, including the model readiness checks.
//! - [`query`] builds the query engine over the stores and maps engine
//!   contexts into the results the panel cites.
//! - [`errors`] maps failures onto the shared [`CommandError`] vocabulary.

mod commands;
mod errors;
mod folder;
mod query;
mod runtime;
mod status;
mod types;

#[cfg(test)]
mod tests;

// The `__cmd__*` and `__tauri_command_name_*` re-exports carry the items the
// `#[tauri::command]` macro generates next to each handler; `generate_handler!`
// resolves them through the same path it is given in `lib.rs`, so they have to
// sit at this module root exactly like the commands do.
pub use commands::{
    __cmd__oracle_ask, __cmd__oracle_doctor, __cmd__oracle_files, __cmd__oracle_index_cancel,
    __cmd__oracle_index_start, __cmd__oracle_model_download_cancel,
    __cmd__oracle_model_download_start, __cmd__oracle_stats, __cmd__oracle_status,
    __cmd__oracle_watch_start, __cmd__oracle_watch_stop, __cmd__oracle_workspace_get,
    __cmd__oracle_workspace_set, __tauri_command_name_oracle_ask,
    __tauri_command_name_oracle_doctor, __tauri_command_name_oracle_files,
    __tauri_command_name_oracle_index_cancel, __tauri_command_name_oracle_index_start,
    __tauri_command_name_oracle_model_download_cancel,
    __tauri_command_name_oracle_model_download_start, __tauri_command_name_oracle_stats,
    __tauri_command_name_oracle_status, __tauri_command_name_oracle_watch_start,
    __tauri_command_name_oracle_watch_stop, __tauri_command_name_oracle_workspace_get,
    __tauri_command_name_oracle_workspace_set, oracle_ask, oracle_doctor, oracle_files,
    oracle_index_cancel, oracle_index_start, oracle_model_download_cancel,
    oracle_model_download_start, oracle_stats, oracle_status, oracle_watch_start,
    oracle_watch_stop, oracle_workspace_get, oracle_workspace_set,
};
pub use folder::{
    __cmd__oracle_ask_folder, __cmd__oracle_folder_status, __tauri_command_name_oracle_ask_folder,
    __tauri_command_name_oracle_folder_status, oracle_ask_folder, oracle_folder_status,
};
pub use runtime::OracleRuntime;
// The types keep their original path at the oracle root; nothing inside the
// crate names them through this re-export, and the module itself is private,
// so the unused-imports lint would otherwise fire on pure surface keeping.
#[allow(unused_imports)]
pub use types::{
    FileTab, IndexedFile, OracleFolderIndexState, OracleFolderIndexStatus, OracleHealth,
    OracleHealthCheck, OracleIndexStats, OracleIndexStatus, OracleMatchType, OracleModelState,
    OracleModelStatus, OracleResourceBudget, OracleResult, OracleSearchResponse, OracleWorkspace,
};
