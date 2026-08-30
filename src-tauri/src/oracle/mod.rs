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
use oracle_core::redact_secret_tokens;
use oracle_core::{
    chunk_index_status, collect_text_files, configured_model_present, configured_reranker_present,
    default_backend, default_model_dir, file_needs_index, index_file_chunks, load_manifest,
    manifest_files_for_root, prune_excluded_chunks, BackendChoice, CancelFlag, ContextChunk,
    EmbedderPool, EpArg, IndexStatusSnapshot, IndexerConfig, LanceStore, OracleDataPaths,
    PoolQueryEmbedder, QueryEngine, RerankerHandle, SharedReranker, SqliteStore, TextEmbedder,
    MAX_BOUNDED_LIMIT,
};

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
    reranker: Mutex<Option<SharedReranker>>,
    model_id: String,
    settings_path: Mutex<Option<PathBuf>>,
    model_download: Arc<Mutex<ModelDownloadState>>,
    reranker_download: Arc<Mutex<ModelDownloadState>>,
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
    fn new(
        model_id: String,
        directory: PathBuf,
        total_files: usize,
        approximate_bytes: u64,
        present: bool,
        component: &str,
    ) -> Self {
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
                total_files,
                bytes_done: 0,
                bytes_total: None,
                approximate_bytes,
                message: Some(if present {
                    format!("Oracle's {component} model is installed.")
                } else {
                    if approximate_bytes > 0 {
                        format!(
                            "Oracle's {component} model `{model_id}` is missing. Oracle looks in {}. The download is about {} MB.",
                            directory.display(),
                            approximate_bytes / 1_000_000
                        )
                    } else {
                        format!(
                            "Oracle's {component} model `{model_id}` is missing. Oracle looks in {}.",
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
    if model_id == oracle_core::BGE_SMALL_MODEL_ID {
        oracle_core::BGE_SMALL_APPROX_BYTES
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
            reranker: Mutex::new(None),
            model_download: Arc::new(Mutex::new(ModelDownloadState::new(
                model_id.clone(),
                PathBuf::new(),
                oracle_core::BGE_SMALL_FILES.len(),
                oracle_core::BGE_SMALL_APPROX_BYTES,
                false,
                "embedding",
            ))),
            reranker_download: Arc::new(Mutex::new(ModelDownloadState::new(
                oracle_core::RERANKER_MODEL_ID.to_string(),
                PathBuf::new(),
                oracle_core::RERANKER_FILES.len(),
                oracle_core::RERANKER_APPROX_BYTES,
                false,
                "reranker",
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
    pub fn load_persisted_root(&self, config_dir: &Path) -> Result<(), CommandError> {
        let settings_path = config_dir.join(ORACLE_SETTINGS_FILE);
        *self
            .settings_path
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(settings_path.clone());

        if std::env::var_os(ORACLE_ROOT_ENV)
            .filter(|value| !value.is_empty())
            .is_some()
        {
            return Ok(());
        }

        let raw = match fs::read_to_string(&settings_path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(core_error(
                    &format!(
                        "reading Oracle preferences at {} failed. Choose the Oracle folder again from the panel",
                        settings_path.display()
                    ),
                    error,
                ));
            }
        };
        let settings = match serde_json::from_str::<PersistedOracleSettings>(&raw) {
            Ok(settings) => settings,
            Err(error) => {
                return Err(invalid_configuration(format!(
                    "Oracle preferences at {} contain invalid JSON: {error}. Choose the Oracle folder again from the panel.",
                    settings_path.display()
                )));
            }
        };
        let root = PathBuf::from(settings.oracle_root);
        if root.as_os_str().is_empty() {
            return Err(invalid_configuration(format!(
                "Oracle preferences at {} contain an empty workspace path. Choose the Oracle folder again from the panel.",
                settings_path.display()
            )));
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
        Ok(())
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
        let model_dir = oracle_core::model_dir_for(&data.root, &self.model_id);
        let pool = Arc::new(EmbedderPool::new(default_backend(model_dir.clone())));
        let reranker_dir = default_model_dir(&data.root);
        let model_present = configured_model_present(&model_dir, true);
        let reranker_present = configured_reranker_present(&reranker_dir);
        let reranker = RerankerHandle::if_present(reranker_dir.clone(), EpArg::Cpu).map(Arc::new);
        let paths = ResolvedOraclePaths {
            workspace: root.clone(),
            data,
        };
        *self.root.lock().unwrap_or_else(|error| error.into_inner()) = Some(root);
        *self.paths.lock().unwrap_or_else(|error| error.into_inner()) = Some(paths);
        *self.pool.lock().unwrap_or_else(|error| error.into_inner()) = Some(pool.clone());
        *self
            .reranker
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = reranker;
        let mut model_download = ModelDownloadState::new(
            self.model_id.clone(),
            model_dir,
            oracle_core::BGE_SMALL_FILES.len(),
            approximate_model_size(&self.model_id),
            model_present,
            "embedding",
        );
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
        *self
            .reranker_download
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = ModelDownloadState::new(
            oracle_core::RERANKER_MODEL_ID.to_string(),
            reranker_dir,
            oracle_core::RERANKER_FILES.len(),
            oracle_core::RERANKER_APPROX_BYTES,
            reranker_present,
            "reranker",
        );
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
        ensure_workspace_accessible(&path)?;
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
        ensure_workspace_readable(&paths.workspace)?;
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

    fn reranker(&self) -> Option<SharedReranker> {
        let mut slot = self
            .reranker
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if slot.is_none() {
            let directory = self
                .reranker_download
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .status
                .directory
                .clone();
            if !directory.is_empty() {
                if let Some(handle) =
                    RerankerHandle::if_present(PathBuf::from(directory), EpArg::Cpu)
                {
                    *slot = Some(Arc::new(handle));
                }
            }
        }
        slot.clone()
    }

    fn model_status(&self) -> OracleModelStatus {
        self.model_download
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .status
            .clone()
    }

    fn reranker_status(&self) -> OracleModelStatus {
        let mut state = self
            .reranker_download
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.status.state != OracleModelState::Downloading
            && configured_reranker_present(Path::new(&state.status.directory))
        {
            state.status.state = OracleModelState::Ready;
            state.status.message = Some("Oracle's reranker model is ready.".to_string());
        }
        state.status.clone()
    }

    fn is_model_downloading(&self) -> bool {
        let embedding_downloading = self
            .model_download
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .status
            .state
            == OracleModelState::Downloading;
        let reranker_downloading = self
            .reranker_download
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .status
            .state
            == OracleModelState::Downloading;
        embedding_downloading || reranker_downloading
    }

    /// Start one descriptor-driven bundle installer. The ensure function owns
    /// HEAD/size verification, `.part` writes, atomic rename, timeouts,
    /// cancellation, and progress for both models.
    fn start_bundle_download(
        &self,
        slot: Arc<Mutex<ModelDownloadState>>,
        descriptor: &'static oracle_core::ModelBundleDescriptor,
        model_dir: PathBuf,
        component: &'static str,
        force: bool,
    ) -> Result<(), CommandError> {
        let progress = Arc::clone(&slot);
        let cancel_slot = Arc::clone(&slot);
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
            let cancel = CancelFlag::new();
            state.attempted = true;
            state.cancel = Some(cancel.clone());
            state.status = OracleModelStatus {
                state: OracleModelState::Downloading,
                model_id: descriptor.model_id.to_string(),
                directory: model_dir.display().to_string(),
                file: None,
                file_index: 0,
                total_files: descriptor.files.len(),
                bytes_done: 0,
                bytes_total: None,
                approximate_bytes: descriptor.approximate_bytes,
                message: Some(format!(
                    "Downloading Oracle's {component} model (about {} MB) from Hugging Face.",
                    descriptor.approximate_bytes / 1_000_000
                )),
            };
            cancel
        };

        let model_id = descriptor.model_id.to_string();
        let total_files = descriptor.files.len();
        let approximate_bytes = descriptor.approximate_bytes;
        let failure_slot = Arc::clone(&slot);
        std::thread::Builder::new()
            .name(format!("oracle-{component}-model-download"))
            .spawn(move || {
                let result = oracle_core::ensure_model_onnx_at_with_cancel(
                    &model_dir,
                    descriptor,
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
                        model_id: model_id.clone(),
                        directory: model_dir.display().to_string(),
                        file: None,
                        file_index: total_files,
                        total_files,
                        bytes_done: 0,
                        bytes_total: None,
                        approximate_bytes,
                        message: Some(format!("Oracle's {component} model is ready.")),
                    },
                    Err(error) if cancelled => OracleModelStatus {
                        state: OracleModelState::Cancelled,
                        model_id: model_id.clone(),
                        directory: model_dir.display().to_string(),
                        file: None,
                        file_index: 0,
                        total_files,
                        bytes_done: 0,
                        bytes_total: None,
                        approximate_bytes,
                        message: Some(format!(
                            "Oracle's {component} model download cancelled ({error}). Start it again from the Oracle panel."
                        )),
                    },
                    Err(error) => OracleModelStatus {
                        state: OracleModelState::Failed,
                        model_id,
                        directory: model_dir.display().to_string(),
                        file: None,
                        file_index: 0,
                        total_files,
                        bytes_done: 0,
                        bytes_total: None,
                        approximate_bytes,
                        message: Some(format!(
                            "Oracle's {component} model download failed: {error:#}. Retry from the Oracle panel; the model is expected at {}.",
                            model_dir.display()
                        )),
                    },
                };
            })
            .map_err(|error| {
                let mut state = failure_slot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.cancel = None;
                state.status.state = OracleModelState::Failed;
                state.status.message = Some(format!(
                    "Could not start Oracle's {component} model download: {error}. Retry from the Oracle panel."
                ));
                CommandError::new(
                    ErrorCode::Io,
                    "Could not start the Oracle model download. Retry from the Oracle panel.",
                )
            })?;
        Ok(())
    }

    /// Start both model transfers in the background. The reranker is optional:
    /// its absence never blocks the dense query path.
    fn start_model_download(&self, force: bool) -> Result<(), CommandError> {
        self.paths()?;
        let pool = self.pool()?;

        if let BackendChoice::Ort { model_dir, .. } = pool.backend() {
            if self.model_id == oracle_core::BGE_SMALL_MODEL_ID {
                self.start_bundle_download(
                    Arc::clone(&self.model_download),
                    &oracle_core::BGE_SMALL_BUNDLE,
                    model_dir.clone(),
                    "embedding",
                    force,
                )?;
            } else {
                let mut state = self
                    .model_download
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if state.status.state != OracleModelState::Downloading
                    && (!state.attempted || force)
                {
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
                }
            }
        }

        let reranker_dir = {
            self.reranker_download
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .status
                .directory
                .clone()
        };
        self.start_bundle_download(
            Arc::clone(&self.reranker_download),
            &oracle_core::RERANKER_BUNDLE,
            PathBuf::from(reranker_dir),
            "reranker",
            force,
        )
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
        for slot in [&self.model_download, &self.reranker_download] {
            if let Some(cancel) = slot
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .cancel
                .clone()
            {
                cancel.cancel();
            }
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

fn ensure_workspace_accessible(path: &Path) -> Result<(), CommandError> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => ensure_workspace_readable(path),
        Ok(_) => Err(invalid_configuration(format!(
            "The selected Oracle workspace is not a folder: {}. Choose an existing folder that is already on disk.",
            path.display()
        ))),
        Err(error) => Err(core_error(
            &format!(
                "accessing the selected Oracle workspace {} failed. Check that the folder exists and that Devboule can read it, then choose another folder",
                path.display()
            ),
            error,
        )),
    }
}

fn ensure_workspace_readable(path: &Path) -> Result<(), CommandError> {
    fs::read_dir(path).map(|_| ()).map_err(|error| {
        core_error(
            &format!(
                "reading Oracle workspace {} failed. Check the folder permissions and choose a readable folder",
                path.display()
            ),
            error,
        )
    })
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
    pub reranker: Option<OracleModelStatus>,
    /// Explanation for an index that is incomplete or currently waiting on a
    /// resource, such as available memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pause_reason: Option<String>,
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
    /// The narrower span inside `[line_start, line_end]` that the cross-encoder
    /// scored as the answer, when it could pick one. It is a suggestion about
    /// where to look first, not a replacement for the range: `snippet` still
    /// carries the whole chunk, so a caller that disagrees loses nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_line_start: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_line_end: Option<usize>,
    pub snippet: String,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_type: Option<OracleMatchType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OracleMatchType {
    #[serde(rename = "lexical")]
    Lexical,
    #[serde(rename = "dense")]
    Dense,
    #[serde(rename = "dense+lexical")]
    DenseLexical,
    #[serde(rename = "dense+reranked")]
    DenseReranked,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleSearchResponse {
    pub query: String,
    pub results: Vec<OracleResult>,
}

fn oracle_workspace_get_inner(runtime: &OracleRuntime) -> Result<OracleWorkspace, CommandError> {
    Ok(runtime.workspace())
}

fn oracle_workspace_set_inner(
    runtime: &OracleRuntime,
    path: &str,
) -> Result<OracleWorkspace, CommandError> {
    runtime.set_workspace(path)
}

fn oracle_model_download_start_inner(runtime: &OracleRuntime) -> Result<(), CommandError> {
    runtime.start_model_download(true)
}

fn oracle_model_download_cancel_inner(runtime: &OracleRuntime) -> Result<(), CommandError> {
    runtime.cancel_model_download();
    Ok(())
}

fn oracle_index_cancel_inner(runtime: &OracleRuntime) -> Result<(), CommandError> {
    runtime.cancel_index();
    Ok(())
}

async fn oracle_status_inner(runtime: &OracleRuntime) -> Result<OracleIndexStatus, CommandError> {
    let paths = runtime.paths()?;
    runtime.start_model_download(false)?;
    let snapshot = read_index_snapshot(&paths).await?;
    Ok(status_from_snapshot(runtime, &snapshot))
}

async fn oracle_doctor_inner(runtime: &OracleRuntime) -> Result<OracleHealth, CommandError> {
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

async fn oracle_stats_inner(runtime: &OracleRuntime) -> Result<OracleIndexStats, CommandError> {
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

async fn oracle_files_inner(
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

async fn oracle_ask_inner(
    runtime: &OracleRuntime,
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
    let engine = open_engine(&paths, runtime.reranker())?;
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

fn oracle_index_start_inner(runtime: &OracleRuntime) -> Result<(), CommandError> {
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

fn run_index_job(
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
    // Not on a cancelled or failed run: a partial index has not seen every file
    // and pruning against it would delete the ones it never reached.
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

async fn read_index_snapshot(
    paths: &ResolvedOraclePaths,
) -> Result<IndexStatusSnapshot, CommandError> {
    let sqlite = SqliteStore::new(&paths.data.metadata)
        .map_err(|error| core_error("opening Oracle metadata store failed", error))?;
    let chunk_vectors = LanceStore::new(&paths.data.chunks);
    chunk_index_status(
        &paths.workspace,
        &sqlite,
        &chunk_vectors,
        &paths.data.manifest,
    )
    .await
    .map_err(|error| core_error("reading Oracle index status failed", error))
}

fn open_engine(
    paths: &ResolvedOraclePaths,
    reranker: Option<SharedReranker>,
) -> Result<QueryEngine, CommandError> {
    let sqlite = SqliteStore::new(&paths.data.metadata)
        .map_err(|error| core_error("opening Oracle metadata store failed", error))?;
    Ok(QueryEngine::new(
        sqlite,
        LanceStore::new(&paths.data.vectors),
        Some(LanceStore::new(&paths.data.chunks)),
        Some(LanceStore::new(&paths.data.file_vectors)),
    )
    .with_reranker(reranker))
}

fn status_from_snapshot(
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

fn result_from_context(root: &Path, context: &ContextChunk) -> OracleResult {
    let (line_start, line_end) = line_range(root, context);
    let (focus_line_start, focus_line_end) = match context.focus {
        Some(focus) => focus_range(line_start, line_end, focus),
        None => (None, None),
    };
    OracleResult {
        path: context.file_source.clone(),
        line_start,
        line_end,
        focus_line_start,
        focus_line_end,
        snippet: redact_secret_tokens(&context.text),
        score: context.score,
        symbol_name: (!context.symbol_name.is_empty()).then(|| context.symbol_name.clone()),
        match_type: match context.retrieval.as_str() {
            "lexical" => Some(OracleMatchType::Lexical),
            "dense" => Some(OracleMatchType::Dense),
            "dense+lexical" => Some(OracleMatchType::DenseLexical),
            "dense+reranked" => Some(OracleMatchType::DenseReranked),
            _ => None,
        },
    }
}

/// Turn a chunk-relative focus window into absolute file lines.
///
/// The engine reports the window as an offset into the chunk text because only
/// this layer knows the chunk's line base: code chunks carry it in the index,
/// prose chunks have it derived from character offsets just above. A chunk with
/// no known base (both ends zero) gets no focus rather than a guessed one, and
/// a window that would fall outside the chunk's own range is dropped for the
/// same reason — a citation that cannot be trusted is worse than a wide one.
fn focus_range(
    line_start: usize,
    line_end: usize,
    focus: oracle_core::FocusSpan,
) -> (Option<usize>, Option<usize>) {
    if line_start == 0 || focus.line_count == 0 {
        return (None, None);
    }
    let start = line_start.saturating_add(focus.line_offset);
    if start > line_end {
        return (None, None);
    }
    let end = start
        .saturating_add(focus.line_count.saturating_sub(1))
        .min(line_end);
    (Some(start), Some(end))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex as StdMutex, MutexGuard, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    static ENV_LOCK: OnceLock<StdMutex<()>> = OnceLock::new();

    #[test]
    fn result_mapping_preserves_the_reranked_match_type() {
        let context = ContextChunk {
            chunk_id: "chunk-1".to_string(),
            file_source: "src/lib.rs".to_string(),
            chunk_index: 0,
            start_char: 0,
            end_char: 10,
            score: 0.5,
            rerank_score: Some(0.9),
            focus: None,
            retrieval: "dense+reranked".to_string(),
            text: "fn answer() {}".to_string(),
            last_modified: String::new(),
            kind: "function".to_string(),
            symbol_name: "answer".to_string(),
            signature: String::new(),
            language: "rust".to_string(),
            line_start: 1,
            line_end: 1,
            symbols_used: Vec::new(),
        };

        let result = result_from_context(Path::new("."), &context);
        assert_eq!(result.match_type, Some(OracleMatchType::DenseReranked));
        assert_eq!(
            serde_json::to_value(&result).unwrap()["match_type"],
            "dense+reranked"
        );
    }

    #[test]
    fn status_exposes_a_missing_optional_reranker() {
        let _env = TestEnvironment::new("candle");
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = OracleRuntime::from_environment();
        runtime.configure_root(temp.path().to_path_buf());

        let status = runtime.reranker_status();
        assert_eq!(status.state, OracleModelState::Missing);
        assert_eq!(status.model_id, oracle_core::RERANKER_MODEL_ID);
        assert_eq!(status.total_files, oracle_core::RERANKER_FILES.len());
        assert_eq!(status.approximate_bytes, oracle_core::RERANKER_APPROX_BYTES);
        assert!(status.message.unwrap().contains("reranker"));
    }

    #[test]
    fn status_exposes_pending_files_as_an_incomplete_index() {
        let _env = TestEnvironment::new("candle");
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = OracleRuntime::from_environment();
        runtime.configure_root(temp.path().to_path_buf());
        let snapshot = IndexStatusSnapshot {
            root: temp.path().display().to_string(),
            manifest_path: temp.path().join("manifest.json").display().to_string(),
            expected_files: 200,
            indexed_files: 80,
            pending_files: 120,
            stale_files: 0,
            sqlite_chunk_files: 80,
            sqlite_chunks: 160,
            vector_records: 160,
            chunk_profile: "test".to_string(),
            first_pending: Vec::new(),
            first_stale: Vec::new(),
            free_gb: 10.0,
            pause_reason: Some(
                "Oracle paused indexing because available memory is low.".to_string(),
            ),
        };

        let status = status_from_snapshot(&runtime, &snapshot);
        assert_eq!(status.state, "incomplete");
        assert_eq!(status.indexed_files, 80);
        assert_eq!(status.pending_files, 120);
        assert_eq!(
            status.pause_reason.as_deref(),
            Some("Oracle paused indexing because available memory is low.")
        );
    }

    struct TestEnvironment {
        _lock: MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<OsString>)>,
    }

    impl TestEnvironment {
        fn new(backend: &str) -> Self {
            let lock = ENV_LOCK
                .get_or_init(|| StdMutex::new(()))
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let keys = [
                ORACLE_ROOT_ENV,
                ORACLE_MODEL_ENV,
                "ORACLE_DIR",
                "ORACLE_RS_BACKEND",
                "ORACLE_RS_EP",
                "ORACLE_RERANKER_MODEL_DIR",
                "ORACLE_RERANK_CANDIDATES",
                "ORACLE_RERANK_BATCH_SIZE",
                "ORACLE_CHUNK_MIN_FREE_RAM_GB",
                "ORACLE_CHUNK_MIN_FREE_GB",
                "ORACLE_CHUNK_BATCH_FILES",
                "ORACLE_CHUNK_BATCH_CHARS",
                "ORACLE_CHUNK_ATTENTION_BUDGET",
            ];
            let saved = keys
                .iter()
                .map(|key| (*key, std::env::var_os(key)))
                .collect();
            for key in keys {
                std::env::remove_var(key);
            }
            std::env::set_var("ORACLE_RS_BACKEND", backend);
            Self { _lock: lock, saved }
        }

        fn set(&self, key: &'static str, value: impl AsRef<std::ffi::OsStr>) {
            std::env::set_var(key, value);
        }
    }

    impl Drop for TestEnvironment {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    fn runtime_with_config(config_dir: &Path) -> OracleRuntime {
        let runtime = OracleRuntime::from_environment();
        runtime
            .load_persisted_root(config_dir)
            .expect("missing preferences are a valid first-run state");
        runtime
    }

    fn assert_actionable(error: CommandError, fragments: &[&str]) {
        let message = error.message.to_lowercase();
        for fragment in fragments {
            assert!(
                message.contains(&fragment.to_lowercase()),
                "error message did not contain {fragment:?}: {}",
                error.message
            );
        }
    }

    fn resolved_paths(root: &Path) -> ResolvedOraclePaths {
        ResolvedOraclePaths {
            workspace: root.to_path_buf(),
            data: OracleDataPaths::from_root(root),
        }
    }

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

        let status_error = tauri::async_runtime::block_on(oracle_status_inner(&runtime))
            .expect_err("no workspace");
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
        let selected_workspace = oracle_workspace_set_inner(&runtime, selected.to_str().unwrap())
            .expect("choose workspace");
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

        let files =
            tauri::async_runtime::block_on(oracle_files_inner(&runtime, FileTab::Pending, 1))
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

    #[test]
    fn second_model_download_start_does_not_replace_an_in_flight_state() {
        let env = TestEnvironment::new("onnx");
        env.set(ORACLE_MODEL_ENV, "model-without-an-installer");
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = OracleRuntime::from_environment();
        runtime.configure_root(temp.path().to_path_buf());
        {
            let mut state = runtime
                .model_download
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.status.state = OracleModelState::Downloading;
            state.cancel = Some(CancelFlag::new());
            state.attempted = true;
        }

        oracle_model_download_start_inner(&runtime).expect("second start is an idempotent no-op");
        let state = runtime
            .model_download
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(state.status.state, OracleModelState::Downloading);
        assert!(state.cancel.is_some());
        assert!(state.attempted);
    }

    #[test]
    fn index_start_rejects_a_second_run() {
        let _env = TestEnvironment::new("candle");
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = OracleRuntime::from_environment();
        runtime.configure_root(temp.path().to_path_buf());
        runtime.indexing.store(true, Ordering::Release);

        let error = oracle_index_start_inner(&runtime).expect_err("second index must fail");
        assert_actionable(error, &["already running"]);
        runtime.indexing.store(false, Ordering::Release);
    }

    struct SlowTestEmbedder {
        started: Arc<AtomicBool>,
    }

    impl TextEmbedder for SlowTestEmbedder {
        fn model_id(&self) -> anyhow::Result<String> {
            Ok("oracle-command-test-model".to_string())
        }

        fn dims(&self) -> anyhow::Result<usize> {
            Ok(4)
        }

        fn embed(
            &self,
            texts: &[String],
            _batch_size: usize,
            _cancel: &CancelFlag,
        ) -> anyhow::Result<Vec<Vec<f32>>> {
            self.started.store(true, Ordering::Release);
            thread::sleep(Duration::from_millis(100));
            Ok(texts.iter().map(|_| vec![1.0, 0.0, 0.0, 0.0]).collect())
        }

        fn embedding_recipe(&self) -> anyhow::Result<String> {
            Ok("oracle-command-test-recipe".to_string())
        }
    }

    #[test]
    fn index_cancel_reaches_the_real_indexer_loop() {
        let env = TestEnvironment::new("candle");
        env.set("ORACLE_CHUNK_MIN_FREE_RAM_GB", "0");
        env.set("ORACLE_CHUNK_BATCH_FILES", "1");
        env.set("ORACLE_CHUNK_BATCH_CHARS", "10000000");
        env.set("ORACLE_CHUNK_ATTENTION_BUDGET", "1000000000");

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let file_count = 24;
        for index in 0..file_count {
            fs::write(
                root.join(format!("slow-{index}.txt")),
                format!("cancellation sentinel file {index}\n"),
            )
            .expect("test source file");
        }

        let runtime = OracleRuntime::from_environment();
        let paths = resolved_paths(&root);
        let cancel = CancelFlag::new();
        let started = Arc::new(AtomicBool::new(false));
        let embedder: Arc<dyn TextEmbedder> = Arc::new(SlowTestEmbedder {
            started: Arc::clone(&started),
        });
        let indexing = Arc::clone(&runtime.indexing);
        let index_cancel = Arc::clone(&runtime.index_cancel);
        let last_index_error = Arc::clone(&runtime.last_index_error);
        indexing.store(true, Ordering::Release);
        *index_cancel
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(cancel.clone());

        let worker = thread::spawn(move || {
            run_index_job(
                paths,
                embedder,
                indexing,
                index_cancel,
                last_index_error,
                cancel,
            );
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        while !started.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            started.load(Ordering::Acquire),
            "index worker never embedded"
        );
        oracle_index_cancel_inner(&runtime).expect("cancel command");
        worker.join().expect("index worker should not panic");

        let sqlite =
            SqliteStore::new(&OracleDataPaths::from_root(&root).metadata).expect("metadata store");
        assert!(
            sqlite.chunk_file_count().expect("chunk file count") < file_count,
            "index cancellation must stop the worker before all files are committed"
        );
        assert!(!runtime.indexing.load(Ordering::Acquire));
        assert!(runtime
            .index_cancel
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_none());
        assert!(runtime.index_error().is_none());
    }

    /// Real model proof. Run manually from the repository root with:
    ///
    ///     cargo test -p devboule --lib oracle::tests::real_model_choose_index_query -- --ignored --nocapture
    ///
    /// The test uses `recon/models/bge-small-en-v1.5` by default (or the path
    /// in `DEVBOULE_E2E_MODEL_DIR`) and is ignored because the sandbox cannot
    /// link or execute the local ONNX Runtime reliably.
    #[test]
    #[ignore]
    fn real_model_choose_index_query() {
        let env = TestEnvironment::new("onnx");
        env.set("ORACLE_RS_EP", "cpu");
        let source = std::env::var_os("DEVBOULE_E2E_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join("..")
                    .join("recon")
                    .join("models")
                    .join(DEFAULT_ORACLE_MODEL)
            });
        let source = source
            .canonicalize()
            .unwrap_or_else(|error| panic!("real model directory {}: {error}", source.display()));
        for required in [
            "model_config.json",
            "tokenizer.json",
            "onnx/model_quantized.onnx",
        ] {
            assert!(
                source.join(required).is_file(),
                "real model is missing {} under {}",
                required,
                source.display()
            );
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let model_target = OracleDataPaths::from_root(&root)
            .root
            .join("models")
            .join(DEFAULT_ORACLE_MODEL);
        for required in [
            "model_config.json",
            "tokenizer.json",
            "onnx/model_quantized.onnx",
        ] {
            let target = model_target.join(required);
            fs::create_dir_all(target.parent().expect("model parent")).expect("model parent");
            fs::copy(source.join(required), &target).expect("copy real model asset");
        }

        let deployment = root.join("src").join("zephyr_release.rs");
        fs::create_dir_all(deployment.parent().expect("source parent")).expect("source parent");
        fs::write(
            &deployment,
            "pub fn reconcile_zephyr_release() {\n    // The release gate records the heliograph attestation in the deployment ledger.\n}\n",
        )
        .expect("deployment source");
        fs::write(
            root.join("cooking.txt"),
            "A sourdough starter needs flour, water, and time before baking.\n",
        )
        .expect("decoy text");
        fs::write(
            root.join("billing.txt"),
            "Invoices are collected from a saved card at the end of each cycle.\n",
        )
        .expect("decoy text");

        // The settings file must live outside the indexed workspace, as it does
        // in production (`app_config_dir()`). Putting it under `root` made the
        // indexer pick up `config/oracle-settings.json` as a project file.
        let config_home = tempfile::tempdir().expect("config tempdir");
        let config = config_home.path().join("config");
        let runtime = runtime_with_config(&config);
        let chosen = oracle_workspace_set_inner(&runtime, root.to_str().unwrap())
            .expect("choose temporary workspace");
        let chosen_path = PathBuf::from(chosen.path.as_deref().expect("chosen path"));
        assert!(chosen_path.is_absolute());
        assert_eq!(chosen_path.file_name(), root.file_name());
        wait_for_model_ready(&runtime);
        assert!(configured_model_present(&model_target, true));

        oracle_index_start_inner(&runtime).expect("start real index");
        wait_for_index(&runtime);
        let status = tauri::async_runtime::block_on(oracle_status_inner(&runtime))
            .expect("status after real index");
        assert_eq!(status.state, "ready");
        assert_eq!(status.indexed_files, 3);

        let indexed =
            tauri::async_runtime::block_on(oracle_files_inner(&runtime, FileTab::Indexed, 1))
                .expect("indexed files");
        assert!(indexed
            .iter()
            .any(|file| file.path == "src/zephyr_release.rs"));

        let response = tauri::async_runtime::block_on(oracle_ask_inner(
            &runtime,
            "Where is the heliograph attestation recorded for the release gate?".to_string(),
        ))
        .expect("real Oracle query");
        assert!(
            !response.results.is_empty(),
            "real query returned no results"
        );
        assert_eq!(
            response.results[0].path,
            "src/zephyr_release.rs",
            "real query returned the wrong top file: {:?}",
            response
                .results
                .iter()
                .map(|result| &result.path)
                .collect::<Vec<_>>()
        );
        assert!(response
            .results
            .iter()
            .any(|result| result.path == "src/zephyr_release.rs"));
    }

    /// Real-repository proof through the product runtime and command paths.
    /// Run from PowerShell with:
    ///
    ///     $env:ORACLE_REAL_REPO_ROOT='C:\\path\\to\\repo'; $env:ORACLE_REAL_REPO_QUERIES='C:\\path\\to\\queries.json'; cargo test -p devboule --lib oracle::tests::real_repo_index_and_query -- --ignored --nocapture
    ///
    /// The workspace is supplied by `ORACLE_REAL_REPO_ROOT`; only Oracle's
    /// data/config directories are temporary. The test is ignored because the
    /// sandbox cannot link or execute the local ONNX Runtime reliably.
    #[test]
    #[ignore]
    fn real_repo_index_and_query() {
        let Some(workspace) = std::env::var_os("ORACLE_REAL_REPO_ROOT")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
        else {
            println!("skipping real_repo_index_and_query: ORACLE_REAL_REPO_ROOT is not set");
            return;
        };
        let Some(queries_path) = std::env::var_os("ORACLE_REAL_REPO_QUERIES")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
        else {
            println!("skipping real_repo_index_and_query: ORACLE_REAL_REPO_QUERIES is not set");
            return;
        };

        let env = TestEnvironment::new("onnx");
        env.set("ORACLE_RS_EP", "cpu");
        let workspace = workspace
            .canonicalize()
            .unwrap_or_else(|error| panic!("real repository {}: {error}", workspace.display()));
        let queries_path = queries_path.canonicalize().unwrap_or_else(|error| {
            panic!(
                "real repository query file {}: {error}",
                queries_path.display()
            )
        });
        assert!(
            workspace.is_dir(),
            "real repository root is not a directory: {}",
            workspace.display()
        );
        assert!(
            queries_path.is_file(),
            "real repository query file is not a file: {}",
            queries_path.display()
        );

        let queries: Vec<RealRepoQuery> =
            serde_json::from_str(&fs::read_to_string(&queries_path).unwrap_or_else(|error| {
                panic!(
                    "reading real repository query file {} failed: {error}",
                    queries_path.display()
                )
            }))
            .unwrap_or_else(|error| {
                panic!(
                    "parsing real repository query file {} failed: {error}",
                    queries_path.display()
                )
            });

        let data_home = tempfile::tempdir().expect("Oracle data tempdir");
        env.set("ORACLE_DIR", data_home.path());
        let model_sources = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("recon")
            .join("models");
        let reranker_model_id = "ms-marco-TinyBERT-L-2-v2";
        copy_model_bundle(
            &model_sources.join(DEFAULT_ORACLE_MODEL),
            &data_home.path().join("models").join(DEFAULT_ORACLE_MODEL),
            &[
                "model_config.json",
                "tokenizer.json",
                "onnx/model_quantized.onnx",
            ],
        );
        copy_model_bundle(
            &model_sources.join(reranker_model_id),
            &data_home.path().join("models").join(reranker_model_id),
            &[
                "model_config.json",
                "tokenizer.json",
                "onnx/model_int8.onnx",
            ],
        );

        let config_home = tempfile::tempdir().expect("Oracle config tempdir");
        let runtime = runtime_with_config(&config_home.path().join("config"));
        let chosen = oracle_workspace_set_inner(&runtime, workspace.to_str().unwrap())
            .expect("choose real repository workspace");
        assert_eq!(
            chosen.path.as_deref(),
            Some(workspace.to_str().expect("workspace path is UTF-8"))
        );
        wait_for_model_ready(&runtime);
        assert_eq!(
            runtime.model_status().state,
            OracleModelState::Ready,
            "real embedder model did not become ready: {:?}",
            runtime.model_status()
        );

        let indexing_started = Instant::now();
        oracle_index_start_inner(&runtime).expect("start real repository index");
        wait_for_index(&runtime);
        let indexing_elapsed = indexing_started.elapsed();
        let status = tauri::async_runtime::block_on(oracle_status_inner(&runtime))
            .expect("status after real repository index");
        let stats = tauri::async_runtime::block_on(oracle_stats_inner(&runtime))
            .expect("stats after real repository index");
        println!(
            "real repository indexed in {:?}: {} files, {} chunks",
            indexing_elapsed, stats.indexed_files, stats.indexed_chunks
        );
        assert_eq!(status.state, "ready", "real repository index is not ready");
        assert_eq!(
            stats.pending_files, 0,
            "real repository index has pending files"
        );
        assert_eq!(
            stats.stale_files, 0,
            "real repository index has stale files"
        );
        assert!(
            stats.indexed_chunks > 0,
            "real repository index produced no chunks"
        );

        let mut top1 = 0;
        let mut top5 = 0;
        let mut missing = 0;
        let mut focused = 0;
        let mut unfocused = 0;
        for (index, query) in queries.iter().enumerate() {
            let expected = normalize_expected_path(&workspace, &query.expect);
            let response =
                tauri::async_runtime::block_on(oracle_ask_inner(&runtime, query.q.clone()))
                    .unwrap_or_else(|error| {
                        panic!(
                            "real repository query {} failed: {}",
                            index + 1,
                            error.message
                        )
                    });
            let rank = response
                .results
                .iter()
                .position(|result| result.path == expected)
                .map(|position| position + 1);

            println!("\nquery {}: {}", index + 1, query.q);
            for (position, result) in response.results.iter().take(5).enumerate() {
                let focus = match (result.focus_line_start, result.focus_line_end) {
                    (Some(start), Some(end)) => {
                        assert!(
                            start >= result.line_start && end <= result.line_end && start <= end,
                            "focus {start}-{end} escapes the cited range {}-{} for {}",
                            result.line_start,
                            result.line_end,
                            result.path
                        );
                        focused += 1;
                        format!(" -> start at {start}-{end}")
                    }
                    (None, None) => {
                        unfocused += 1;
                        String::new()
                    }
                    _ => panic!("half a focus span on {}", result.path),
                };
                println!(
                    "  {}. {} (lines {}-{}){}",
                    position + 1,
                    result.path,
                    result.line_start,
                    result.line_end,
                    focus
                );
            }
            match rank {
                Some(position) => println!("  expected: {} (position {})", expected, position),
                None => {
                    println!("  expected: {} (missing)", expected);
                    missing += 1;
                }
            }
            if rank == Some(1) {
                top1 += 1;
            }
            if matches!(rank, Some(position) if position <= 5) {
                top5 += 1;
            }
        }

        // The reranker was once written, measured, committed and never
        // delivered, because nothing downloaded its model and no test asserted
        // that it had run. The citation focus rides on that same reranker, so
        // this asserts the focus arrived rather than printing a line range that
        // looks identical whether it did or not.
        assert!(
            focused > 0,
            "no result carried a focus span across {} queries. The reranker model is \
             staged by this test, so either the narrowing never ran or every retrieved \
             chunk was too short to narrow — both are regressions",
            queries.len()
        );
        println!("\nfocus: {focused} results narrowed, {unfocused} left at chunk width");

        // The code-knowledge graph has the same failure mode as the reranker
        // had: a store that exists, is exported, and is empty looks exactly like
        // a store that works. Assert against real data rather than a row count,
        // by asking a question whose answer is knowable from this repository.
        {
            let paths = runtime.paths().expect("oracle paths after indexing");
            let ckg = oracle_core::CkgStore::new(&paths.data.ckg).expect("opening the ckg store");
            let engine_file = "crates/oracle-core/src/query/engine.rs";
            let imports = ckg
                .imports_of(engine_file)
                .expect("reading imports out of the ckg");
            let targets: Vec<&str> = imports.iter().map(|edge| edge.dst.as_str()).collect();
            println!("\nckg: {engine_file} imports {} files", targets.len());
            for target in &targets {
                println!("  -> {target}");
            }
            assert!(
                !targets.is_empty(),
                "the graph has no imports for {engine_file}, which uses `crate::` on \
                 several lines: either nothing built the graph or resolution is broken"
            );
            for expected in [
                "crates/oracle-core/src/query/focus.rs",
                "crates/oracle-core/src/query/reranker.rs",
            ] {
                assert!(
                    targets.contains(&expected),
                    "the graph is missing the edge {engine_file} -> {expected}"
                );
            }
            // A neighbourhood walk must reach further than one hop, otherwise
            // the recursive query is returning direct edges and nothing else.
            let reach = ckg
                .neighborhood(engine_file, 2, Some("IMPORT"))
                .expect("walking the ckg");
            println!("ckg: {} files within two imports", reach.len());
            assert!(
                reach.iter().any(|(_, depth)| *depth == 2),
                "no node sits two imports away, so the recursive walk is not walking"
            );
        }

        let outside_top5 = queries.len().saturating_sub(top5 + missing);
        println!(
            "\nsummary: first={}/{}, top5={}/{}, missing={}/{}, outside_top5={}/{}",
            top1,
            queries.len(),
            top5,
            queries.len(),
            missing,
            queries.len(),
            outside_top5,
            queries.len()
        );
    }

    #[derive(Debug, Deserialize)]
    struct RealRepoQuery {
        q: String,
        expect: String,
    }

    fn copy_model_bundle(source: &Path, target: &Path, files: &[&str]) {
        let source = source
            .canonicalize()
            .unwrap_or_else(|error| panic!("real model directory {}: {error}", source.display()));
        for relative in files {
            let source_file = source.join(relative);
            let target_file = target.join(relative);
            assert!(
                source_file.is_file(),
                "real model is missing {} under {}",
                relative,
                source.display()
            );
            fs::create_dir_all(target_file.parent().expect("model parent")).expect("model parent");
            fs::copy(&source_file, &target_file).unwrap_or_else(|error| {
                panic!(
                    "copying real model asset {} to {} failed: {error}",
                    source_file.display(),
                    target_file.display()
                )
            });
        }
    }

    fn normalize_expected_path(workspace: &Path, expected: &str) -> String {
        let expected = PathBuf::from(expected);
        let expected = if expected.is_absolute() {
            expected
                .strip_prefix(workspace)
                .unwrap_or(expected.as_path())
                .to_path_buf()
        } else {
            expected
        };
        expected.to_string_lossy().replace('\\', "/")
    }

    fn wait_for_model_ready(runtime: &OracleRuntime) {
        let deadline = Instant::now() + Duration::from_secs(180);
        while runtime.model_status().state == OracleModelState::Downloading {
            assert!(Instant::now() < deadline, "model download did not finish");
            thread::sleep(Duration::from_millis(100));
        }
        assert!(
            matches!(
                runtime.model_status().state,
                OracleModelState::Ready | OracleModelState::Failed
            ),
            "unexpected model state: {:?}",
            runtime.model_status().state
        );
    }

    fn wait_for_index(runtime: &OracleRuntime) {
        let deadline = Instant::now() + Duration::from_secs(300);
        while runtime.indexing.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "real index did not finish");
            thread::sleep(Duration::from_millis(100));
        }
        assert!(
            runtime.index_error().is_none(),
            "real index failed: {:?}",
            runtime.index_error()
        );
    }

    struct UnreadableDirectory {
        path: PathBuf,
        #[cfg(windows)]
        user: String,
        #[cfg(unix)]
        original_mode: std::fs::Permissions,
    }

    impl UnreadableDirectory {
        fn new(path: &Path) -> Self {
            #[cfg(windows)]
            {
                let user = String::from_utf8(
                    std::process::Command::new("whoami")
                        .output()
                        .expect("whoami")
                        .stdout,
                )
                .expect("whoami output")
                .trim()
                .to_string();
                let deny = format!("{user}:(OI)(CI)(RX)");
                let result = std::process::Command::new("icacls")
                    .args([path.as_os_str(), OsStr::new("/deny"), OsStr::new(&deny)])
                    .status()
                    .expect("icacls");
                assert!(result.success(), "icacls failed to deny directory access");
                Self {
                    path: path.to_path_buf(),
                    user,
                }
            }

            #[cfg(unix)]
            {
                let metadata = fs::metadata(path).expect("directory metadata");
                let original_mode = metadata.permissions();
                let mut denied = original_mode.clone();
                use std::os::unix::fs::PermissionsExt;
                denied.set_mode(0);
                fs::set_permissions(path, denied).expect("remove directory permissions");
                Self {
                    path: path.to_path_buf(),
                    original_mode,
                }
            }

            #[cfg(not(any(unix, windows)))]
            {
                panic!("no portable unreadable-directory test implementation");
            }
        }
    }

    impl Drop for UnreadableDirectory {
        fn drop(&mut self) {
            #[cfg(windows)]
            {
                let result = std::process::Command::new("icacls")
                    .args([
                        self.path.as_os_str(),
                        OsStr::new("/remove:d"),
                        OsStr::new(&self.user),
                    ])
                    .status()
                    .expect("icacls restore");
                assert!(
                    result.success(),
                    "icacls failed to restore directory access"
                );
            }

            #[cfg(unix)]
            fs::set_permissions(&self.path, self.original_mode.clone())
                .expect("restore directory permissions");
        }
    }
}
