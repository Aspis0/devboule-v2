//! Oracle's runtime state: the workspace root and how it was configured, the
//! resolved data paths, the embedder pool and reranker, and the model download
//! state machine that both commands and app startup drive.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use devboule_protocol::ErrorCode;
use oracle_core::configured_model_present;
use oracle_core::configured_reranker_present;
use oracle_core::{
    default_backend, default_model_dir, BackendChoice, CancelFlag, EmbedderPool, EpArg,
    OracleDataPaths, RerankerHandle, SharedReranker,
};

use crate::backend::error::CommandError;

use super::errors::{core_error, invalid_configuration};
use super::types::{OracleModelState, OracleModelStatus, OracleWorkspace};

pub(super) const ORACLE_ROOT_ENV: &str = "DEVBOULE_ORACLE_ROOT";
// Developer-only bundle selector. The panel currently exposes the workspace
// choice only; a user-facing model selector can be added independently later.
pub(super) const ORACLE_MODEL_ENV: &str = "DEVBOULE_ORACLE_MODEL";
pub(super) const DEFAULT_ORACLE_MODEL: &str = "bge-small-en-v1.5";
pub(super) const ORACLE_SETTINGS_FILE: &str = "oracle-settings.json";

#[derive(Debug, Clone)]
pub(super) struct ResolvedOraclePaths {
    pub(super) workspace: PathBuf,
    pub(super) data: OracleDataPaths,
}

pub struct OracleRuntime {
    root: Mutex<Option<PathBuf>>,
    root_source: Mutex<String>,
    paths: Mutex<Option<ResolvedOraclePaths>>,
    pool: Mutex<Option<Arc<EmbedderPool>>>,
    reranker: Mutex<Option<SharedReranker>>,
    model_id: String,
    settings_path: Mutex<Option<PathBuf>>,
    pub(super) model_download: Arc<Mutex<ModelDownloadState>>,
    reranker_download: Arc<Mutex<ModelDownloadState>>,
    pub(super) indexing: Arc<AtomicBool>,
    pub(super) index_cancel: Arc<Mutex<Option<CancelFlag>>>,
    pub(super) last_index_error: Arc<Mutex<Option<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PersistedOracleSettings {
    pub(super) oracle_root: String,
}

pub(super) struct ModelDownloadState {
    pub(super) status: OracleModelStatus,
    pub(super) cancel: Option<CancelFlag>,
    pub(super) attempted: bool,
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

    pub(super) fn configure_root(&self, root: PathBuf) {
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

    pub(super) fn set_workspace(&self, requested: &str) -> Result<OracleWorkspace, CommandError> {
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

    pub(super) fn paths(&self) -> Result<ResolvedOraclePaths, CommandError> {
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

    pub(super) fn pool(&self) -> Result<Arc<EmbedderPool>, CommandError> {
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

    pub(super) fn reranker(&self) -> Option<SharedReranker> {
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

    pub(super) fn model_status(&self) -> OracleModelStatus {
        self.model_download
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .status
            .clone()
    }

    pub(super) fn reranker_status(&self) -> OracleModelStatus {
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
    pub(super) fn start_model_download(&self, force: bool) -> Result<(), CommandError> {
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

    pub(super) fn cancel_model_download(&self) {
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

    pub(super) fn cancel_index(&self) {
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

    pub(super) fn index_error(&self) -> Option<String> {
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
