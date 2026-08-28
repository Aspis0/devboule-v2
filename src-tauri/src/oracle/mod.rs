//! Tauri commands for the local Oracle index and query runtime.

use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::State;

use devboule_protocol::ErrorCode;
use oracle_core::config::{OracleDataPaths, MAX_BOUNDED_LIMIT};
use oracle_core::embed::{default_backend, BackendChoice, CancelFlag, EmbedderPool};
use oracle_core::ingest::indexer::{self, IndexStatusSnapshot};
use oracle_core::query::engine::{ContextChunk, QueryEngine};
use oracle_core::query::pool_embedder::PoolQueryEmbedder;
use oracle_core::query::redact::redact_secret_tokens;
use oracle_core::store::lance::LanceStore;
use oracle_core::store::manifest::{self, load_manifest, manifest_files_for_root};
use oracle_core::store::sqlite::SqliteStore;

use crate::backend::error::CommandError;

const ORACLE_ROOT_ENV: &str = "DEVBOULE_ORACLE_ROOT";
const PAGE_SIZE: usize = 50;
const QUERY_LIMIT: usize = 10;

#[derive(Debug, Clone)]
struct ResolvedOraclePaths {
    workspace: PathBuf,
    data: OracleDataPaths,
}

pub struct OracleRuntime {
    root: Option<PathBuf>,
    paths: Option<ResolvedOraclePaths>,
    pool: Option<Arc<EmbedderPool>>,
    indexing: Arc<AtomicBool>,
    last_index_error: Arc<Mutex<Option<String>>>,
}

impl OracleRuntime {
    /// Read the workspace only from explicit app configuration. A relative
    /// value is retained so commands can report the configuration error
    /// instead of resolving it against the process working directory.
    pub fn from_environment() -> Self {
        let root = std::env::var_os(ORACLE_ROOT_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let paths = root.as_ref().filter(|path| path.is_absolute()).map(|root| {
            let data = OracleDataPaths::from_root(root);
            ResolvedOraclePaths {
                workspace: root.clone(),
                data,
            }
        });
        let pool = paths.as_ref().map(|paths| {
            let model_dir =
                oracle_core::embed::ort_backend::OrtEmbedder::default_model_dir(&paths.data.root);
            Arc::new(EmbedderPool::new(default_backend(model_dir)))
        });

        Self {
            root,
            paths,
            pool,
            indexing: Arc::new(AtomicBool::new(false)),
            last_index_error: Arc::new(Mutex::new(None)),
        }
    }

    fn paths(&self) -> Result<ResolvedOraclePaths, CommandError> {
        let Some(root) = self.root.as_ref() else {
            return Err(invalid_configuration(
                "Oracle workspace is not configured. Set DEVBOULE_ORACLE_ROOT to an absolute workspace path in the app configuration.",
            ));
        };
        if !root.is_absolute() {
            return Err(invalid_configuration(
                "Oracle workspace configuration must be an absolute path; DEVBOULE_ORACLE_ROOT is relative.",
            ));
        }
        let Some(paths) = self.paths.as_ref() else {
            return Err(invalid_configuration(
                "Oracle workspace configuration could not be resolved.",
            ));
        };
        if !paths.workspace.is_dir() {
            return Err(invalid_configuration(
                "The configured Oracle workspace does not exist or is not a directory.",
            ));
        }
        Ok(paths.clone())
    }

    fn pool(&self) -> Result<Arc<EmbedderPool>, CommandError> {
        self.pool.clone().ok_or_else(|| {
            invalid_configuration(
                "Oracle embedding is unavailable until DEVBOULE_ORACLE_ROOT is configured.",
            )
        })
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
pub async fn oracle_status(
    runtime: State<'_, OracleRuntime>,
) -> Result<OracleIndexStatus, CommandError> {
    let paths = runtime.paths()?;
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
                    "Configure DEVBOULE_ORACLE_ROOT before using the Oracle workspace.".to_string(),
                ),
            });
        }
    };

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
    checks.push(model_health_check(pool.backend()));

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
    ensure_model_is_available(pool.backend())?;

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
    let last_index_error = Arc::clone(&runtime.last_index_error);
    let thread_result = std::thread::Builder::new()
        .name("oracle-index".to_string())
        .spawn(move || {
            let sqlite = match SqliteStore::new(&paths.data.metadata) {
                Ok(sqlite) => sqlite,
                Err(error) => {
                    finish_index(
                        &indexing,
                        &last_index_error,
                        Some(format!("opening Oracle metadata store failed: {error}")),
                    );
                    return;
                }
            };
            let chunk_vectors = LanceStore::new(&paths.data.chunks);
            let cancel = CancelFlag::new();
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
                Ok(Ok(_)) => finish_index(&indexing, &last_index_error, None),
                Ok(Err(error)) => finish_index(
                    &indexing,
                    &last_index_error,
                    Some(format!("Oracle indexing failed: {error}")),
                ),
                Err(_) => finish_index(
                    &indexing,
                    &last_index_error,
                    Some("Oracle indexing task panicked.".to_string()),
                ),
            }
        });

    if let Err(error) = thread_result {
        runtime.indexing.store(false, Ordering::Release);
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
    last_index_error: &Mutex<Option<String>>,
    error: Option<String>,
) {
    *last_index_error
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = error;
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

fn model_health_check(backend: &BackendChoice) -> OracleHealthCheck {
    match backend {
        BackendChoice::Ort { model_dir, int8 } => {
            if oracle_core::model_download::model_present_at(model_dir, *int8) {
                health_check(
                    "embedder",
                    "ok",
                    Some("The ONNX embedding model is installed."),
                )
            } else {
                health_check(
                    "embedder",
                    "failed",
                    Some("The configured ONNX embedding model is not installed."),
                )
            }
        }
        BackendChoice::Candle { .. } => health_check(
            "embedder",
            "unknown",
            Some("Candle checks its model cache when the model is first loaded."),
        ),
    }
}

fn ensure_model_is_available(backend: &BackendChoice) -> Result<(), CommandError> {
    if let BackendChoice::Ort { model_dir, int8 } = backend {
        if !oracle_core::model_download::model_present_at(model_dir, *int8) {
            return Err(invalid_configuration(
                "Oracle embedding model is not installed. Install the Qwen3 ONNX runtime before indexing.",
            ));
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
