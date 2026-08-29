//! Tauri commands for the local Oracle index and query runtime.

use std::fmt::Display;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::State;

use devboule_protocol::ErrorCode;
use oracle_core::config::{OracleDataPaths, MAX_BOUNDED_LIMIT};
use oracle_core::embed::{
    configured_model_present, default_backend, BackendChoice, CancelFlag, EmbedderPool,
};
use oracle_core::ingest::indexer::{self, IndexStatusSnapshot};
use oracle_core::query::engine::{ContextChunk, QueryEngine};
use oracle_core::query::pool_embedder::PoolQueryEmbedder;
use oracle_core::query::redact::redact_secret_tokens;
use oracle_core::store::lance::LanceStore;
use oracle_core::store::manifest::{self, load_manifest, manifest_files_for_root};
use oracle_core::store::sqlite::SqliteStore;

use crate::backend::error::CommandError;

const ORACLE_ROOT_ENV: &str = "DEVBOULE_ORACLE_ROOT";
// Developer-only bundle selector. The panel currently exposes the workspace
// choice only; a user-facing model selector can be added independently later.
const ORACLE_MODEL_ENV: &str = "DEVBOULE_ORACLE_MODEL";
const DEFAULT_ORACLE_MODEL: &str = "bge-small-en-v1.5";
const ORACLE_SETTINGS_FILE: &str = "oracle-settings.json";
const PAGE_SIZE: usize = 50;
const QUERY_LIMIT: usize = 10;

#[derive(Debug, Clone)]
struct ResolvedOraclePaths {
    workspace: PathBuf,
    data: OracleDataPaths,
}

pub struct OracleRuntime {
    root: Mutex<Option<PathBuf>>,
    root_source: Mutex<String>,
    paths: Mutex<Option<ResolvedOraclePaths>>,
    pool: Mutex<Option<Arc<EmbedderPool>>>,
    model_id: String,
    settings_path: Mutex<Option<PathBuf>>,
    model_download: Arc<Mutex<ModelDownloadState>>,
    indexing: Arc<AtomicBool>,
    index_cancel: Arc<Mutex<Option<CancelFlag>>>,
    last_index_error: Arc<Mutex<Option<String>>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleWorkspace {
    pub path: Option<String>,
    pub source: String,
    pub exists: bool,
    pub editable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleModelState {
    NotApplicable,
    Missing,
    Downloading,
    Ready,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleModelStatus {
    pub state: OracleModelState,
    pub model_id: String,
    pub directory: String,
    pub file: Option<String>,
    pub file_index: usize,
    pub total_files: usize,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub approximate_bytes: u64,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedOracleSettings {
    oracle_root: String,
}

struct ModelDownloadState {
    status: OracleModelStatus,
    cancel: Option<CancelFlag>,
    attempted: bool,
}

impl ModelDownloadState {
    fn new(model_id: String, directory: PathBuf) -> Self {
        let present =
            !directory.as_os_str().is_empty() && configured_model_present(&directory, true);
        let approximate_bytes = approximate_model_size(&model_id);
        Self {
            status: OracleModelStatus {
                state: if present {
                    OracleModelState::Ready
                } else {
                    OracleModelState::Missing
                },
                model_id: model_id.clone(),
                directory: directory.display().to_string(),
                file: None,
                file_index: 0,
                total_files: oracle_core::model_download::BGE_SMALL_FILES.len(),
                bytes_done: 0,
                bytes_total: None,
                approximate_bytes,
                message: Some(if present {
                    "Oracle's embedding model is installed.".to_string()
                } else {
                    if approximate_bytes > 0 {
                        format!(
                            "Model `{model_id}` is missing. Oracle looks in {}. The download is about {} MB.",
                            directory.display(),
                            approximate_bytes / 1_000_000
                        )
                    } else {
                        format!(
                            "Model `{model_id}` is missing. Oracle looks in {}.",
                            directory.display()
                        )
                    }
                }),
            },
            cancel: None,
            attempted: false,
        }
    }
}

fn configured_model_id() -> String {
    std::env::var(ORACLE_MODEL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ORACLE_MODEL.to_string())
        .trim()
        .to_string()
}

fn approximate_model_size(model_id: &str) -> u64 {
    if model_id == oracle_core::model_download::BGE_SMALL_MODEL_ID {
        oracle_core::model_download::BGE_SMALL_APPROX_BYTES
    } else {
        0
    }
}

impl OracleRuntime {
    /// Read the workspace only from explicit app configuration. A relative
    /// value is retained so commands can report the configuration error
    /// instead of resolving it against the process working directory.
    pub fn from_environment() -> Self {
        let root = std::env::var_os(ORACLE_ROOT_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let model_id = configured_model_id();
        let runtime = Self {
            root: Mutex::new(None),
            root_source: Mutex::new("unset".to_string()),
            paths: Mutex::new(None),
            pool: Mutex::new(None),
            model_download: Arc::new(Mutex::new(ModelDownloadState::new(
                model_id.clone(),
                PathBuf::new(),
            ))),
            model_id,
            settings_path: Mutex::new(None),
            indexing: Arc::new(AtomicBool::new(false)),
            index_cancel: Arc::new(Mutex::new(None)),
            last_index_error: Arc::new(Mutex::new(None)),
        };

        if let Some(root) = root {
            *runtime
                .root_source
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = "environment".to_string();
            if root.is_absolute() {
                runtime.configure_root(root);
            } else {
                *runtime
                    .root
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = Some(root);
            }
        }
        runtime
    }

    /// Load Oracle's one persisted panel preference. The developer env var is
    /// intentionally checked first and always wins over this file.
    pub fn load_persisted_root(&self, config_dir: &Path) {
        let settings_path = config_dir.join(ORACLE_SETTINGS_FILE);
        *self
            .settings_path
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(settings_path.clone());

        if std::env::var_os(ORACLE_ROOT_ENV)
            .filter(|value| !value.is_empty())
            .is_some()
        {
            return;
        }

        let raw = match fs::read_to_string(&settings_path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                eprintln!(
                    "devboule: cannot read Oracle preferences at {}: {error}. Choose the Oracle folder again from the panel.",
                    settings_path.display()
                );
                return;
            }
        };
        let settings = match serde_json::from_str::<PersistedOracleSettings>(&raw) {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!(
                    "devboule: Oracle preferences at {} are invalid JSON: {error}. Choose the Oracle folder again from the panel.",
                    settings_path.display()
                );
                return;
            }
        };
        let root = PathBuf::from(settings.oracle_root);
        if root.as_os_str().is_empty() {
            eprintln!(
                "devboule: Oracle preferences at {} contain an empty workspace path. Choose the Oracle folder again from the panel.",
                settings_path.display()
            );
            return;
        }
        *self
            .root_source
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = "saved".to_string();
        if root.is_absolute() {
            self.configure_root(root);
        } else {
            *self.root.lock().unwrap_or_else(|error| error.into_inner()) = Some(root);
        }
    }

    pub fn workspace(&self) -> OracleWorkspace {
        let path = self
            .root
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let source = self
            .root_source
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        OracleWorkspace {
            exists: path.as_ref().is_some_and(|path| path.is_dir()),
            editable: source != "environment",
            path: path.map(|path| path.to_string_lossy().into_owned()),
            source,
        }
    }

    fn configure_root(&self, root: PathBuf) {
        let data = OracleDataPaths::from_root(&root);
        let model_dir = oracle_core::model_download::model_dir_for(&data.root, &self.model_id);
        let pool = Arc::new(EmbedderPool::new(default_backend(model_dir.clone())));
        let paths = ResolvedOraclePaths {
            workspace: root.clone(),
            data,
        };
        *self.root.lock().unwrap_or_else(|error| error.into_inner()) = Some(root);
        *self.paths.lock().unwrap_or_else(|error| error.into_inner()) = Some(paths);
        *self.pool.lock().unwrap_or_else(|error| error.into_inner()) = Some(pool.clone());
        let mut model_download = ModelDownloadState::new(self.model_id.clone(), model_dir);
        if matches!(pool.backend(), BackendChoice::Candle { .. }) {
            model_download.status.state = OracleModelState::NotApplicable;
            model_download.status.message = Some(
                "Candle is an explicit developer backend override; it uses its own model cache."
                    .to_string(),
            );
            model_download.attempted = true;
        }
        *self
            .model_download
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = model_download;
    }

    fn persist_root(&self, root: &Path) -> Result<(), CommandError> {
        let settings_path = self
            .settings_path
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .ok_or_else(|| {
                invalid_configuration(
                    "Oracle preferences cannot be saved because the application config directory is unavailable. Choose a folder again after restarting Devboule.",
                )
            })?;
        if let Some(parent) = settings_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                core_error("creating the Oracle preferences directory failed", error)
            })?;
        }
        let settings = PersistedOracleSettings {
            oracle_root: root.to_string_lossy().into_owned(),
        };
        let raw = serde_json::to_vec_pretty(&settings)
            .map_err(|error| core_error("serializing Oracle preferences failed", error))?;
        let parent = settings_path.parent().ok_or_else(|| {
            invalid_configuration(
                "Oracle preferences have no containing directory. Choose a folder again after restarting Devboule.",
            )
        })?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
            core_error(
                "creating the temporary Oracle preferences file failed",
                error,
            )
        })?;
        temp.write_all(&raw).map_err(|error| {
            core_error(
                "writing the temporary Oracle preferences file failed",
                error,
            )
        })?;
        temp.as_file().sync_all().map_err(|error| {
            core_error(
                "flushing the temporary Oracle preferences file failed",
                error,
            )
        })?;
        temp.persist(&settings_path).map_err(|error| {
            core_error(
                "atomically replacing the Oracle preferences file failed",
                error.error,
            )
        })?;
        Ok(())
    }

    fn set_workspace(&self, requested: &str) -> Result<OracleWorkspace, CommandError> {
        if !self.workspace().editable {
            return Err(invalid_configuration(format!(
                "DEVBOULE_ORACLE_ROOT overrides the saved Oracle folder ({}). Unset that developer variable to choose a folder from the panel.",
                self.workspace().path.unwrap_or_default()
            )));
        }
        if self.indexing.load(Ordering::Acquire) {
            return Err(invalid_configuration(
                "Oracle is indexing. Cancel the current index before changing its workspace.",
            ));
        }
        if self.is_model_downloading() {
            return Err(invalid_configuration(
                "Oracle is downloading its model. Cancel the download before changing its workspace.",
            ));
        }

        let path = PathBuf::from(requested.trim());
        if path.as_os_str().is_empty() {
            return Err(invalid_configuration(
                "Choose an Oracle workspace folder; the selected path was empty.",
            ));
        }
        if !path.is_absolute() {
            return Err(invalid_configuration(
                "Choose an absolute Oracle workspace folder, not a relative path.",
            ));
        }
        if !path.is_dir() {
            return Err(invalid_configuration(format!(
                "The selected Oracle workspace is not an existing folder: {}. Choose a folder that is already on disk.",
                path.display()
            )));
        }
        let path = path.canonicalize().map_err(|error| {
            core_error(
                "resolving the selected Oracle workspace failed",
                format!("{} ({error})", path.display()),
            )
        })?;
        self.persist_root(&path)?;
        *self
            .root_source
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = "saved".to_string();
        self.configure_root(path);
        self.start_model_download(false)?;
        Ok(self.workspace())
    }

    fn paths(&self) -> Result<ResolvedOraclePaths, CommandError> {
        let root = self
            .root
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let Some(root) = root.as_ref() else {
            return Err(invalid_configuration(
                "Oracle has no workspace folder. Choose an existing folder in the Oracle panel; developers can alternatively set DEVBOULE_ORACLE_ROOT to an absolute path.",
            ));
        };
        if !root.is_absolute() {
            return Err(invalid_configuration(
                "Oracle workspace must be an absolute path. The DEVBOULE_ORACLE_ROOT developer override is relative; change it to an absolute path or choose a folder in the panel.",
            ));
        }
        let paths = self
            .paths
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let Some(paths) = paths else {
            return Err(invalid_configuration(
                "Oracle workspace configuration could not be resolved.",
            ));
        };
        if !paths.workspace.is_dir() {
            return Err(invalid_configuration(
                format!(
                    "Oracle workspace {} no longer exists or is not a directory. Choose another existing folder in the Oracle panel.",
                    root.display()
                ),
            ));
        }
        Ok(paths.clone())
    }

    fn pool(&self) -> Result<Arc<EmbedderPool>, CommandError> {
        self.pool
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .ok_or_else(|| {
            invalid_configuration(
                "Oracle embedding is unavailable until you choose an existing workspace folder in the Oracle panel.",
            )
        })
    }

    fn model_status(&self) -> OracleModelStatus {
        self.model_download
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .status
            .clone()
    }

    fn is_model_downloading(&self) -> bool {
        self.model_download
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .status
            .state
            == OracleModelState::Downloading
    }

    /// Start the default model installer once per configured workspace. The
    /// ensure function still performs its own HEAD/size verification; the
    /// UI-only presence check is deliberately not used to skip that attempt.
    fn start_model_download(&self, force: bool) -> Result<(), CommandError> {
        let paths = self.paths()?;
        let pool = self.pool()?;
        let BackendChoice::Ort { model_dir, .. } = pool.backend() else {
            return Ok(());
        };

        let progress = Arc::clone(&self.model_download);
        let cancel_slot = Arc::clone(&self.model_download);
        let cancel = {
            let mut state = progress.lock().unwrap_or_else(|error| error.into_inner());
            if state.status.state == OracleModelState::Downloading {
                return Ok(());
            }
            if state.status.state == OracleModelState::Ready && state.attempted {
                return Ok(());
            }
            if state.attempted && !force {
                return Ok(());
            }
            if self.model_id != oracle_core::model_download::BGE_SMALL_MODEL_ID {
                state.attempted = true;
                state.status = OracleModelStatus {
                    state: OracleModelState::Failed,
                    model_id: self.model_id.clone(),
                    directory: model_dir.display().to_string(),
                    file: None,
                    file_index: 0,
                    total_files: 0,
                    bytes_done: 0,
                    bytes_total: None,
                    approximate_bytes: approximate_model_size(&self.model_id),
                    message: Some(format!(
                        "Model `{}` has no automatic installer. Put its declared ONNX bundle in {} or remove {ORACLE_MODEL_ENV} to use the supported BGE model.",
                        self.model_id,
                        model_dir.display()
                    )),
                };
                return Ok(());
            }
            let cancel = CancelFlag::new();
            state.attempted = true;
            state.cancel = Some(cancel.clone());
            state.status = OracleModelStatus {
                state: OracleModelState::Downloading,
                model_id: self.model_id.clone(),
                directory: model_dir.display().to_string(),
                file: None,
                file_index: 0,
                total_files: oracle_core::model_download::BGE_SMALL_FILES.len(),
                bytes_done: 0,
                bytes_total: None,
                approximate_bytes: oracle_core::model_download::BGE_SMALL_APPROX_BYTES,
                message: Some(format!(
                    "Downloading about {} MB from Hugging Face.",
                    oracle_core::model_download::BGE_SMALL_APPROX_BYTES / 1_000_000
                )),
            };
            cancel
        };

        let data_root = paths.data.root.clone();
        let model_id = self.model_id.clone();
        let model_dir = model_dir.clone();
        std::thread::Builder::new()
            .name("oracle-model-download".to_string())
            .spawn(move || {
                let result = oracle_core::model_download::ensure_bge_small_onnx_with_cancel(
                    &data_root,
                    &cancel,
                    |file_progress| {
                        let mut state = progress
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        state.status.file = Some(file_progress.file);
                        state.status.file_index = file_progress.index;
                        state.status.total_files = file_progress.total_files;
                        state.status.bytes_done = file_progress.bytes_done;
                        state.status.bytes_total = file_progress.bytes_total;
                    },
                );
                let mut state = cancel_slot
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let cancelled = cancel.is_cancelled();
                state.cancel = None;
                state.status = match result {
                    Ok(_) => OracleModelStatus {
                        state: OracleModelState::Ready,
                        model_id,
                        directory: model_dir.display().to_string(),
                        file: None,
                        file_index: oracle_core::model_download::BGE_SMALL_FILES.len(),
                        total_files: oracle_core::model_download::BGE_SMALL_FILES.len(),
                        bytes_done: 0,
                        bytes_total: None,
                        approximate_bytes: oracle_core::model_download::BGE_SMALL_APPROX_BYTES,
                        message: Some("Oracle's embedding model is ready.".to_string()),
                    },
                    Err(error) if cancelled => OracleModelStatus {
                        state: OracleModelState::Cancelled,
                        model_id,
                        directory: model_dir.display().to_string(),
                        file: None,
                        file_index: 0,
                        total_files: oracle_core::model_download::BGE_SMALL_FILES.len(),
                        bytes_done: 0,
                        bytes_total: None,
                        approximate_bytes: oracle_core::model_download::BGE_SMALL_APPROX_BYTES,
                        message: Some(format!(
                            "Model download cancelled ({error}). Start it again from the Oracle panel."
                        )),
                    },
                    Err(error) => OracleModelStatus {
                        state: OracleModelState::Failed,
                        model_id,
                        directory: model_dir.display().to_string(),
                        file: None,
                        file_index: 0,
                        total_files: oracle_core::model_download::BGE_SMALL_FILES.len(),
                        bytes_done: 0,
                        bytes_total: None,
                        approximate_bytes: oracle_core::model_download::BGE_SMALL_APPROX_BYTES,
                        message: Some(format!(
                            "Model download failed: {error:#}. Retry from the Oracle panel; the model is expected at {}.",
                            model_dir.display()
                        )),
                    },
                };
            })
            .map_err(|error| {
                let mut state = self
                    .model_download
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.cancel = None;
                state.status.state = OracleModelState::Failed;
                state.status.message = Some(format!(
                    "Could not start the model download: {error}. Retry from the Oracle panel."
                ));
                CommandError::new(
                    ErrorCode::Io,
                    "Could not start the Oracle model download. Retry from the Oracle panel.",
                )
            })?;
        Ok(())
    }

    pub(crate) fn start_model_download_for_startup(&self) -> Result<(), CommandError> {
        if self
            .root
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_none()
        {
            return Ok(());
        }
        self.start_model_download(false)
    }

    fn cancel_model_download(&self) {
        if let Some(cancel) = self
            .model_download
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .cancel
            .clone()
        {
            cancel.cancel();
        }
    }

    fn cancel_index(&self) {
        if let Some(cancel) = self
            .index_cancel
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        {
            cancel.cancel();
        }
    }

    pub fn shutdown(&self) {
        self.cancel_index();
        self.cancel_model_download();
    }

    fn index_error(&self) -> Option<String> {
        self.last_index_error
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleResourceBudget {
    pub max_cpu_percent: f64,
    pub max_memory_mb: f64,
    pub max_parallelism: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleIndexStatus {
    pub state: String,
    pub indexed_files: usize,
    pub total_files: usize,
    pub indexed_chunks: usize,
    pub pending_files: usize,
    pub stale_files: usize,
    pub resource_budget: OracleResourceBudget,
    pub model: OracleModelStatus,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleHealthCheck {
    pub id: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleHealth {
    pub state: String,
    pub checks: Vec<OracleHealthCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleIndexStats {
    pub indexed_files: usize,
    pub indexed_chunks: usize,
    pub pending_files: usize,
    pub stale_files: usize,
    pub backend: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileTab {
    Indexed,
    Pending,
    Stale,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct IndexedFile {
    pub path: String,
    pub chunks: usize,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleResult {
    pub path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub snippet: String,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_type: Option<OracleMatchType>,
}

#[derive(Debug, Serialize)]
pub enum OracleMatchType {
    #[serde(rename = "lexical")]
    Lexical,
    #[serde(rename = "dense")]
    Dense,
    #[serde(rename = "dense+lexical")]
    DenseLexical,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleSearchResponse {
    pub query: String,
    pub results: Vec<OracleResult>,
}

#[tauri::command]
pub fn oracle_workspace_get(
    runtime: State<'_, OracleRuntime>,
) -> Result<OracleWorkspace, CommandError> {
    Ok(runtime.workspace())
}

#[tauri::command]
pub fn oracle_workspace_set(
    runtime: State<'_, OracleRuntime>,
    path: String,
) -> Result<OracleWorkspace, CommandError> {
    runtime.set_workspace(&path)
}

#[tauri::command]
pub fn oracle_model_download_start(runtime: State<'_, OracleRuntime>) -> Result<(), CommandError> {
    runtime.start_model_download(true)
}

#[tauri::command]
pub fn oracle_model_download_cancel(runtime: State<'_, OracleRuntime>) -> Result<(), CommandError> {
    runtime.cancel_model_download();
    Ok(())
}

#[tauri::command]
pub fn oracle_index_cancel(runtime: State<'_, OracleRuntime>) -> Result<(), CommandError> {
    runtime.cancel_index();
    Ok(())
}

#[tauri::command]
pub async fn oracle_status(
    runtime: State<'_, OracleRuntime>,
) -> Result<OracleIndexStatus, CommandError> {
    let paths = runtime.paths()?;
    runtime.start_model_download(false)?;
    let snapshot = read_index_snapshot(&paths).await?;
    Ok(status_from_snapshot(&runtime, &snapshot))
}

#[tauri::command]
pub async fn oracle_doctor(
    runtime: State<'_, OracleRuntime>,
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

    let query_check = match open_engine(&paths) {
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

#[tauri::command]
pub async fn oracle_stats(
    runtime: State<'_, OracleRuntime>,
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

#[tauri::command]
pub fn oracle_index_start(runtime: State<'_, OracleRuntime>) -> Result<(), CommandError> {
    let paths = runtime.paths()?;
    let pool = runtime.pool()?;
    runtime.start_model_download(false)?;
    ensure_model_is_available(pool.backend(), &runtime.model_status())?;

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
            let config = indexer::IndexerConfig::default();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tauri::async_runtime::block_on(indexer::index_file_chunks(
                    &paths.workspace,
                    &sqlite,
                    &chunk_vectors,
                    &paths.data.manifest,
                    pool.as_ref(),
                    &cancel,
                    &config,
                    None,
                ))
            }));
            match result {
                Ok(Ok(_)) => {
                    finish_index(&indexing, &index_cancel, &last_index_error, &cancel, None)
                }
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
    let paths = runtime.paths()?;
    let sqlite = SqliteStore::new(&paths.data.metadata)
        .map_err(|error| core_error("opening Oracle metadata store failed", error))?;
    let mut manifest = load_manifest(&paths.data.manifest);
    let entries = manifest_files_for_root(&mut manifest, &paths.workspace, false)
        .cloned()
        .unwrap_or_default();

    let mut files = Vec::new();
    for path in oracle_core::ingest::collect::collect_text_files(&paths.workspace) {
        let file_id = match path.strip_prefix(&paths.workspace) {
            Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        let entry = entries.get(&file_id);
        let is_pending = entry.is_none();
        let is_stale = match entry {
            Some(_) => manifest::file_needs_index(&path, &paths.workspace, &entries, &sqlite)
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

#[tauri::command]
pub async fn oracle_ask(
    runtime: State<'_, OracleRuntime>,
    query: String,
) -> Result<OracleSearchResponse, CommandError> {
    let query = query.trim().to_string();
    if query.is_empty() {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            "Oracle query cannot be empty.",
        ));
    }
    if query.chars().count() > 4096 {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            "Oracle query is too long (maximum 4096 characters).",
        ));
    }

    let paths = runtime.paths()?;
    let engine = open_engine(&paths)?;
    let pool = runtime.pool()?;
    runtime.start_model_download(false)?;
    ensure_model_is_available(pool.backend(), &runtime.model_status())?;
    let cancel = CancelFlag::new();
    let embedder = PoolQueryEmbedder::new(pool.as_ref(), &cancel)
        .map_err(|error| core_error("initializing Oracle query embedder failed", error))?;
    let contexts = engine
        .context(
            &query,
            QUERY_LIMIT.min(MAX_BOUNDED_LIMIT),
            &embedder,
            None,
            false,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .map_err(|error| core_error("Oracle query failed", error))?;

    let results = contexts
        .iter()
        .map(|context| result_from_context(&paths.workspace, context))
        .collect();
    Ok(OracleSearchResponse { query, results })
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

async fn read_index_snapshot(
    paths: &ResolvedOraclePaths,
) -> Result<IndexStatusSnapshot, CommandError> {
    let sqlite = SqliteStore::new(&paths.data.metadata)
        .map_err(|error| core_error("opening Oracle metadata store failed", error))?;
    let chunk_vectors = LanceStore::new(&paths.data.chunks);
    indexer::chunk_index_status(
        &paths.workspace,
        &sqlite,
        &chunk_vectors,
        &paths.data.manifest,
    )
    .await
    .map_err(|error| core_error("reading Oracle index status failed", error))
}

fn open_engine(paths: &ResolvedOraclePaths) -> Result<QueryEngine, CommandError> {
    let sqlite = SqliteStore::new(&paths.data.metadata)
        .map_err(|error| core_error("opening Oracle metadata store failed", error))?;
    Ok(QueryEngine::new(
        sqlite,
        LanceStore::new(&paths.data.vectors),
        Some(LanceStore::new(&paths.data.chunks)),
        Some(LanceStore::new(&paths.data.file_vectors)),
    ))
}

fn status_from_snapshot(
    runtime: &OracleRuntime,
    snapshot: &IndexStatusSnapshot,
) -> OracleIndexStatus {
    let state = if runtime.indexing.load(Ordering::Acquire) {
        "indexing"
    } else if runtime.index_error().is_some() {
        "error"
    } else if snapshot.pending_files > 0 || snapshot.stale_files > 0 {
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
    }
}

fn result_from_context(root: &Path, context: &ContextChunk) -> OracleResult {
    let (line_start, line_end) = line_range(root, context);
    OracleResult {
        path: context.file_source.clone(),
        line_start,
        line_end,
        snippet: redact_secret_tokens(&context.text),
        score: context.score,
        symbol_name: (!context.symbol_name.is_empty()).then(|| context.symbol_name.clone()),
        match_type: match context.retrieval.as_str() {
            "lexical" => Some(OracleMatchType::Lexical),
            "dense" => Some(OracleMatchType::Dense),
            "dense+lexical" => Some(OracleMatchType::DenseLexical),
            _ => None,
        },
    }
}

fn line_range(root: &Path, context: &ContextChunk) -> (usize, usize) {
    if context.line_start > 0 || context.line_end > 0 {
        return (context.line_start, context.line_end.max(context.line_start));
    }
    let relative = Path::new(&context.file_source);
    if relative.is_absolute() {
        return (0, 0);
    }
    let Ok(source) = std::fs::read_to_string(root.join(relative)) else {
        return (0, 0);
    };
    let start = floor_char_boundary(&source, context.start_char.min(source.len()));
    let end = floor_char_boundary(&source, context.end_char.min(source.len()));
    if end < start {
        return (0, 0);
    }
    let line_start = source[..start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let line_end = source[..end].bytes().filter(|byte| *byte == b'\n').count() + 1;
    (line_start, line_end.max(line_start))
}

fn floor_char_boundary(value: &str, index: usize) -> usize {
    let mut index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn model_health_check(
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

fn ensure_model_is_available(
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

fn backend_label(backend: &BackendChoice) -> String {
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

fn health_check(id: &str, state: &str, message: Option<&str>) -> OracleHealthCheck {
    OracleHealthCheck {
        id: id.to_string(),
        state: state.to_string(),
        message: message.map(str::to_string),
    }
}

fn invalid_configuration(message: impl Into<String>) -> CommandError {
    CommandError::new(ErrorCode::InvalidRequest, message)
}

fn unimplemented_command(message: impl Into<String>) -> CommandError {
    CommandError::new(ErrorCode::Unimplemented, message)
}

fn core_error(context: &str, error: impl Display) -> CommandError {
    CommandError::new(ErrorCode::Internal, format!("{context}: {error}"))
}
