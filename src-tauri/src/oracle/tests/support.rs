//! Shared test harness for the Oracle tests: the environment lock and
//! override helper, temp runtimes, the unreadable-directory fixture, and the
//! wait/copy helpers the end-to-end tests stage real models with.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use oracle_core::{CancelFlag, OracleDataPaths, TextEmbedder};
use serde::Deserialize;

use crate::backend::error::CommandError;
use crate::oracle::runtime::{
    OracleRuntime, ResolvedOraclePaths, ORACLE_MODEL_ENV, ORACLE_ROOT_ENV,
};
use crate::oracle::OracleModelState;

static ENV_LOCK: OnceLock<StdMutex<()>> = OnceLock::new();

pub(super) struct TestEnvironment {
    _lock: MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl TestEnvironment {
    pub(super) fn new(backend: &str) -> Self {
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

    pub(super) fn set(&self, key: &'static str, value: impl AsRef<std::ffi::OsStr>) {
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

pub(super) fn runtime_with_config(config_dir: &Path) -> OracleRuntime {
    let runtime = OracleRuntime::from_environment();
    runtime
        .load_persisted_root(config_dir)
        .expect("missing preferences are a valid first-run state");
    runtime
}

pub(super) fn assert_actionable(error: CommandError, fragments: &[&str]) {
    let message = error.message.to_lowercase();
    for fragment in fragments {
        assert!(
            message.contains(&fragment.to_lowercase()),
            "error message did not contain {fragment:?}: {}",
            error.message
        );
    }
}

pub(super) fn resolved_paths(root: &Path) -> ResolvedOraclePaths {
    ResolvedOraclePaths {
        workspace: root.to_path_buf(),
        data: OracleDataPaths::from_root(root),
    }
}

pub(super) struct SlowTestEmbedder {
    pub(super) started: Arc<AtomicBool>,
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

pub(super) fn wait_for_model_ready(runtime: &OracleRuntime) {
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

pub(super) fn wait_for_index(runtime: &OracleRuntime) {
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

#[derive(Debug, Deserialize)]
pub(super) struct RealRepoQuery {
    pub(super) q: String,
    pub(super) expect: String,
}

pub(super) fn copy_model_bundle(source: &Path, target: &Path, files: &[&str]) {
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

pub(super) fn normalize_expected_path(workspace: &Path, expected: &str) -> String {
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

pub(super) struct UnreadableDirectory {
    path: PathBuf,
    #[cfg(windows)]
    user: String,
    #[cfg(unix)]
    original_mode: std::fs::Permissions,
}

impl UnreadableDirectory {
    pub(super) fn new(path: &Path) -> Self {
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
