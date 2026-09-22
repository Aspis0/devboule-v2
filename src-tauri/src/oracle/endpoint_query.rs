//! The query route behind the bearer gate: parse the daemon's JSON, then
//! refuse fail-closed — model, index, vectors, warm-up — in that order, and
//! only hand a fully qualified search to the engine.
//!
//! The workspace root comes from the request (the daemon resolved it from the
//! session row, never from a model); store paths are built with
//! `from_root_without_env`, so no process environment variable can redirect
//! which workspace's index this reads. The probe only inspects paths: the
//! stores behind it create files on open, and a refusal must leave the
//! caller's folder untouched.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use serde::{Deserialize, Serialize};
use tauri::Manager;

use oracle_core::{EmbedderPool, OracleDataPaths, SharedReranker};

use crate::backend::error::CommandError;

use super::errors::invalid_configuration;
use super::folder::{ensure_folder_index_is_usable, probe_folder_at, Artifact, FolderIndexProbe};
use super::query::{search_paths, validate_query, QUERY_LIMIT};
use super::runtime::{OracleRuntime, ResolvedOraclePaths};
use super::status::ensure_model_is_available;
use super::types::{OracleModelStatus, OracleSearchResponse};

/// Honest while the pool loads: never F1, which would tell a caller to open
/// an app that is already open (DECISIONS Q1).
const WARMING: &str =
    "Oracle's embedding model is still loading in the background. Retry this query in a few seconds.";

/// What the route needs from the application around it. Production wires the
/// app state; tests wire a runtime they own plus a warm counter and a search
/// they can observe.
pub(super) trait QueryHost: Send + Sync + 'static {
    fn pool(&self) -> Result<Arc<EmbedderPool>, CommandError>;
    fn model_status(&self) -> OracleModelStatus;
    fn reranker(&self) -> Option<SharedReranker>;
    fn is_loaded(&self) -> bool;
    fn warm(&self);
    fn search(
        &self,
        paths: &ResolvedOraclePaths,
        query: &str,
        limit: usize,
    ) -> Result<OracleSearchResponse, CommandError>;
}

pub(super) struct AppHost {
    app: tauri::AppHandle,
    warm_slot: WarmSlot,
}

impl AppHost {
    pub(super) fn new(app: tauri::AppHandle) -> Self {
        Self {
            app,
            warm_slot: WarmSlot::new(),
        }
    }
}

/// The single-flight slot behind the `warming` refusal: one warm at a time,
/// and the claim frees itself on every exit — the load finishing, the load
/// panicking while it holds the claim, or a spawn failing before the thread
/// exists (the closure carrying the guard is dropped inside `spawn`). A bare
/// `store(false)` after the load would miss the panic path and leave every
/// later query answering `warming` for the life of the process. Handles are
/// `Clone`: every clone competes for the same flag behind the `Arc`.
#[derive(Clone)]
pub(super) struct WarmSlot {
    warming: Arc<AtomicBool>,
}

impl WarmSlot {
    pub(super) fn new() -> Self {
        Self {
            warming: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Claim the slot; `None` means a warm is already running.
    pub(super) fn try_begin(&self) -> Option<WarmGuard> {
        self.warming
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| WarmGuard {
                warming: Arc::clone(&self.warming),
            })
    }
}

/// Owns the claim. Dropping it — however the warm ends — frees the slot, so
/// no path through `AppHost::warm` can leave it stuck at `true`.
pub(super) struct WarmGuard {
    warming: Arc<AtomicBool>,
}

impl Drop for WarmGuard {
    fn drop(&mut self) {
        self.warming.store(false, Ordering::Release);
    }
}

impl QueryHost for AppHost {
    fn pool(&self) -> Result<Arc<EmbedderPool>, CommandError> {
        self.app.state::<OracleRuntime>().pool()
    }

    fn model_status(&self) -> OracleModelStatus {
        self.app.state::<OracleRuntime>().model_status()
    }

    fn reranker(&self) -> Option<SharedReranker> {
        self.app.state::<OracleRuntime>().reranker()
    }

    fn is_loaded(&self) -> bool {
        self.pool().is_ok_and(|pool| pool.is_loaded())
    }

    /// Load the model off every request thread. One warm at a time: a second
    /// call while a load runs is answered by the same `warming` refusal. The
    /// guard travels inside the closure, so whichever way the warm ends —
    /// completion, panic, or a spawn whose closure `spawn` itself drops on
    /// the error path — the slot frees itself without a branch here.
    fn warm(&self) {
        let Ok(pool) = self.pool() else {
            return;
        };
        let Some(guard) = self.warm_slot.try_begin() else {
            return;
        };
        let _ = thread::Builder::new()
            .name("oracle-warm".into())
            .spawn(move || {
                let _ = pool.model_metadata();
                drop(guard);
            });
    }

    /// The engine's future is awaited on the calling thread: the accept loop
    /// gives every connection its own thread, so the window's thread never
    /// waits for a query.
    fn search(
        &self,
        paths: &ResolvedOraclePaths,
        query: &str,
        limit: usize,
    ) -> Result<OracleSearchResponse, CommandError> {
        let pool = self.pool()?;
        let reranker = self.reranker();
        let model_status = self.model_status();
        tauri::async_runtime::block_on(search_paths(
            paths,
            query,
            &pool,
            reranker,
            &model_status,
            limit,
        ))
    }
}

#[derive(Deserialize)]
struct WireQuery {
    root: Option<String>,
    query: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug)]
pub(super) struct QueryRequest {
    pub(super) root: PathBuf,
    pub(super) query: String,
    pub(super) limit: usize,
}

/// One query returns at most `QUERY_LIMIT` results: an absent field defaults
/// to `QUERY_LIMIT` and `0` clamps up to `1`. A negative value never lands
/// anywhere — `limit: Option<usize>` rejects it during deserialization and
/// the route answers 400: a malformed body, not a bound to clamp.
pub(super) fn parse_request(raw: &[u8]) -> Result<QueryRequest, CommandError> {
    let wire: WireQuery = serde_json::from_slice(raw).map_err(|error| {
        invalid_configuration(format!("The Oracle query body is not valid JSON: {error}"))
    })?;
    let root = wire
        .root
        .as_deref()
        .map(str::trim)
        .filter(|root| !root.is_empty())
        .ok_or_else(|| invalid_configuration("The Oracle query body needs a workspace root."))?;
    if !Path::new(root).is_absolute() {
        return Err(invalid_configuration(
            "The Oracle query root must be an absolute path.",
        ));
    }
    let query = validate_query(wire.query.unwrap_or_default())?;
    let limit = wire.limit.unwrap_or(QUERY_LIMIT).clamp(1, QUERY_LIMIT);
    Ok(QueryRequest {
        root: PathBuf::from(root),
        query,
        limit,
    })
}

/// Answer one authenticated query. HTTP status 200 carries both results and
/// the refusal envelopes; only a body the route cannot parse gets 400.
pub(super) fn respond(host: &dyn QueryHost, raw: &[u8]) -> (u16, Vec<u8>) {
    let request = match parse_request(raw) {
        Ok(request) => request,
        Err(error) => return (400, json(&BadRequest::new(&error.message))),
    };
    let pool = match host.pool() {
        Ok(pool) => pool,
        // No pool means no workspace is configured yet, and the runtime's
        // sentence says exactly that — so the tag names the workspace, not
        // the model (the model gate below keeps `no_model`).
        Err(error) => return refusal("no_app_workspace", &error.message),
    };
    if let Err(error) = ensure_model_is_available(pool.backend(), &host.model_status()) {
        return refusal("no_model", &error.message);
    }
    let probe = probe_folder_at(
        &request.root,
        OracleDataPaths::from_root_without_env(&request.root),
    );
    if let Err(error) = ensure_folder_index_is_usable(&probe) {
        return refusal("no_index", &error.message);
    }
    if !matches!(probe.chunks, Artifact::Present) {
        return refusal("no_vectors", &no_vectors_message(&probe));
    }
    if !host.is_loaded() {
        host.warm();
        return refusal("warming", WARMING);
    }
    let paths = ResolvedOraclePaths {
        workspace: probe.root.clone(),
        data: probe.data.clone(),
    };
    match host.search(&paths, &request.query, request.limit) {
        Ok(response) => (200, json(&Success::new(&response))),
        Err(error) => refusal("query_failed", &error.message),
    }
}

/// Without the dense path the engine falls back to lexical retrieval in
/// silence (`store/lance.rs`, `query/engine.rs`); an agent would take that
/// for the semantic answer it was promised, so the refusal says so.
pub(super) fn no_vectors_message(probe: &FolderIndexProbe) -> String {
    match &probe.chunks {
        Artifact::Unreadable(error) => format!(
            "Oracle cannot read the chunk vector store {}: {error}. Semantic search would silently fall back to lexical-only results; check the folder permissions, then retry.",
            probe.data.chunks.display()
        ),
        _ => format!(
            "The workspace {} has no chunk vector store ({} does not exist), so semantic search would silently fall back to lexical-only results. Re-index this workspace, then retry.",
            probe.root.display(),
            probe.data.chunks.display()
        ),
    }
}

fn refusal(reason: &str, message: &str) -> (u16, Vec<u8>) {
    (
        200,
        json(&Refusal {
            ok: false,
            reason,
            message,
        }),
    )
}

#[derive(Serialize)]
struct Refusal<'a> {
    ok: bool,
    reason: &'a str,
    message: &'a str,
}

#[derive(Serialize)]
struct Success<'a> {
    ok: bool,
    #[serde(flatten)]
    response: &'a OracleSearchResponse,
}

impl<'a> Success<'a> {
    fn new(response: &'a OracleSearchResponse) -> Self {
        Self { ok: true, response }
    }
}

#[derive(Serialize)]
struct BadRequest<'a> {
    error: &'static str,
    message: &'a str,
}

impl<'a> BadRequest<'a> {
    fn new(message: &'a str) -> Self {
        Self {
            error: "bad request",
            message,
        }
    }
}

/// Messages reach the wire through here, so their quotes and backslashes get
/// escaped exactly once. The inputs are strings; a failure would mean the
/// serializer itself broke, and an envelope that still parses beats silence.
fn json<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_else(|_| {
        br#"{"ok":false,"reason":"query_failed","message":"response serialization failed"}"#
            .to_vec()
    })
}
