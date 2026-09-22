//! The shared pieces of the endpoint tests: the [`QueryHost`] they run the
//! route against, and the wire harness (ephemeral record paths, the record
//! reader, one request/response exchange, response JSON).
//!
//! The host answers pool, model status and reranker from the real runtime,
//! while the warm counter, the loaded flag and the search results belong to
//! the test — a refusal branch must run without ever loading an embedding
//! backend.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devboule_daemon::{oracle_app_lock_path, OracleAppRecord, OracleAppState, RuntimePaths};
use oracle_core::EmbedderPool;

use crate::backend::error::CommandError;
use crate::oracle::endpoint_query::QueryHost;
use crate::oracle::runtime::{OracleRuntime, ResolvedOraclePaths};
use crate::oracle::{OracleModelStatus, OracleResult, OracleSearchResponse};

pub(super) const QUERY_PATH: &str = "/oracle/v1/query";

pub(super) fn unique_paths() -> (RuntimePaths, DirGuard) {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "devboule oracle endpoint {}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let guard = DirGuard(dir.clone());
    (RuntimePaths::from_dir(dir), guard)
}

pub(super) struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The record as a fresh endpoint leaves it: live, bound, carrying the port
/// and token a caller needs.
pub(super) fn published(paths: &RuntimePaths) -> OracleAppRecord {
    match OracleAppState::read(&oracle_app_lock_path(paths)) {
        OracleAppState::Live(record) => record,
        other => panic!("expected a live record right after start, got {other:?}"),
    }
}

/// Send one request and return (status code, full response). The server
/// closes the connection after answering, which ends the read.
pub(super) fn send(
    port: u16,
    method: &str,
    path: &str,
    authorization: &str,
    body: &str,
) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: {authorization}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("write");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|token| token.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status line in {response:?}"));
    (status, response)
}

pub(super) fn response_json(response: &str) -> serde_json::Value {
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_else(|| panic!("no body in {response:?}"));
    serde_json::from_str(body).unwrap_or_else(|error| panic!("body is not JSON ({error}): {body}"))
}

pub(super) fn query_body(root: &Path, query: &str, limit: usize) -> String {
    serde_json::json!({
        "root": root.display().to_string(),
        "query": query,
        "limit": limit
    })
    .to_string()
}

#[derive(Clone)]
pub(super) struct Searched {
    pub(super) data_root: PathBuf,
    pub(super) query: String,
    pub(super) limit: usize,
}

pub(super) struct TestHost {
    runtime: Arc<OracleRuntime>,
    loaded: AtomicBool,
    warm_calls: AtomicUsize,
    canned: Mutex<Vec<OracleResult>>,
    searched: Mutex<Option<Searched>>,
}

impl TestHost {
    pub(super) fn new(runtime: OracleRuntime) -> Arc<Self> {
        Arc::new(Self {
            runtime: Arc::new(runtime),
            loaded: AtomicBool::new(false),
            warm_calls: AtomicUsize::new(0),
            canned: Mutex::new(Vec::new()),
            searched: Mutex::new(None),
        })
    }

    /// A host with no configured workspace: `pool()` fails the way a
    /// first-run app does, which the route answers as `no_app_workspace`.
    pub(super) fn bare() -> Arc<Self> {
        Self::new(OracleRuntime::from_environment())
    }

    pub(super) fn set_loaded(&self, loaded: bool) {
        self.loaded.store(loaded, Ordering::Release);
    }

    pub(super) fn serve_results(&self, results: Vec<OracleResult>) {
        *self
            .canned
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = results;
    }

    pub(super) fn warm_calls(&self) -> usize {
        self.warm_calls.load(Ordering::Acquire)
    }

    pub(super) fn searched(&self) -> Option<Searched> {
        self.searched
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

impl QueryHost for TestHost {
    fn pool(&self) -> Result<Arc<EmbedderPool>, CommandError> {
        self.runtime.pool()
    }

    fn model_status(&self) -> OracleModelStatus {
        self.runtime.model_status()
    }

    fn reranker(&self) -> Option<oracle_core::SharedReranker> {
        self.runtime.reranker()
    }

    fn is_loaded(&self) -> bool {
        self.loaded.load(Ordering::Acquire)
    }

    fn warm(&self) {
        self.warm_calls.fetch_add(1, Ordering::AcqRel);
    }

    fn search(
        &self,
        paths: &ResolvedOraclePaths,
        query: &str,
        limit: usize,
    ) -> Result<OracleSearchResponse, CommandError> {
        *self
            .searched
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(Searched {
            data_root: paths.data.root.clone(),
            query: query.to_string(),
            limit,
        });
        let results = self
            .canned
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        Ok(OracleSearchResponse {
            query: query.to_string(),
            results,
        })
    }
}
