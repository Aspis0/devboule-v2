//! Codex app-server stdio transport for live agent sessions.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use devboule_protocol::{ErrorCode, NoticeSeverity, SessionEvent, WireError};
use serde_json::Value;

use super::permission_broker::{PermissionBroker, PermissionSender};
use super::session_runtime::SessionRuntime;
use super::{
    write_child_stdin, ModelSwitcher, PtyCommand, ReaderDispatch, SessionKiller, SessionSteerer,
    SpawnedSession, StderrSource, StdioWaitableChild, TurnToken,
};
use crate::attachment_store::AttachmentStore;
use crate::codex_view::{
    catalog_from_response, mode_values, thread_mode_values, validate_mode, CodexCatalog,
    CodexState, CodexStdout, MAX_LINE_BYTES,
};
use crate::paths::RuntimePaths;
use crate::process_tree::{JobObject, ProcessHandle};
use crate::profile_delivery::ProfileDelivery;
use crate::server::ServerState;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const KILL_GRACE: Duration = Duration::from_secs(2);
/// How long one `turn/steer` may wait for its response before the steer's fate
/// is reported as unknown. The wait happens outside the runtime's turn-hold (see
/// `CodexSteerer::steer_active_turn`), so this bound is what stops a silent
/// app-server from parking a steer forever.
const STEER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

/// The requests this client is waiting on answers for, keyed by the JSON-RPC id
/// the answer will name (A2-03).
///
/// Codex answers `turn/steer` with a response frame, and the response is the
/// only thing that can turn the steer into a truthful `Ok(true)`: writing the
/// request is not Codex taking it.
/// One request's answer channel, in the shape both failures share: `Ok(value)` is
/// the response, `Err(message)` is the transport ending before one came (S4-04).
/// Named, so no signature has to carry the `Result` in a `Result`.
type RequestAnswer = mpsc::Receiver<Result<Value, String>>;

/// The sender end of one [`RequestAnswer`].
type RequestAnswerSender = mpsc::Sender<Result<Value, String>>;

#[derive(Default)]
pub(crate) struct CodexRequests {
    pending: Mutex<HashMap<String, RequestAnswerSender>>,
}

impl CodexRequests {
    fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Register the channel one response id will be delivered on. `None` when
    /// the map cannot be locked — the caller answers `Err`, never `Ok(true)`.
    fn register(&self, id: &str) -> Option<RequestAnswer> {
        let (tx, rx) = mpsc::channel();
        self.pending
            .lock()
            .ok()
            .map(|mut pending| pending.insert(id.to_string(), tx))?;
        Some(rx)
    }

    /// Drop one registration: the request failed to write, or its answer never
    /// came and the waiter is giving up on it.
    fn forget(&self, id: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(id);
        }
    }

    /// How many waiters are parked (test-only): lets a test wait until a
    /// verification has registered before failing it, so the transport-end
    /// case is deterministic instead of racy.
    #[cfg(test)]
    fn pending_count(&self) -> usize {
        self.pending
            .lock()
            .map(|pending| pending.len())
            .unwrap_or(0)
    }

    /// Hand one response to the waiter that registered its id.
    ///
    /// A response whose id matches no waiter is *ignored*, deliberately: the id
    /// space carries Codex's answers to every request this client makes (the
    /// handshake's, the writer's `turn/start`, a `turn/steer` whose waiter
    /// already timed out), and a response for none of them is not an error and
    /// must not take anything down.
    fn deliver(&self, value: &Value) -> bool {
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            return false;
        };
        let sender = self
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(id));
        sender.is_some_and(|sender| sender.send(Ok(value.clone())).is_ok())
    }

    /// Wake every waiter still registered with the reason the control channel
    /// ended (S4-04).
    ///
    /// A response can no longer arrive for any of them — the app-server's output
    /// is what delivers responses, and it is over — so a waiter left registered
    /// would sit out its whole timeout for an answer that cannot come. The map is
    /// drained under its lock and each sender is answered outside it.
    fn fail_pending(&self, message: &str) {
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        let waiters: Vec<RequestAnswerSender> = pending.drain().map(|(_, sender)| sender).collect();
        drop(pending);
        for sender in waiters {
            let _ = sender.send(Err(message.to_string()));
        }
    }
}

pub(super) fn resolve_command(paths: &RuntimePaths) -> Result<PtyCommand, WireError> {
    let cwd = std::env::current_dir().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not determine Codex working directory: {error}"),
        )
    })?;
    let Some(agent) = crate::provider_catalog::find_available("codex") else {
        return Err(WireError::new(
            ErrorCode::Io,
            "Codex was not found on PATH.",
        ));
    };
    let _ = paths;
    let Some(mut argv) = agent.app_server_command else {
        return Err(WireError::new(
            ErrorCode::Io,
            "Codex is installed but has no app-server launch args.",
        ));
    };
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(WireError::new(
            ErrorCode::Io,
            "Codex resolved to an empty command.",
        ));
    }
    let program = argv.remove(0);
    Ok(PtyCommand::new(program, argv, cwd, Vec::new()).with_provider_id("codex"))
}

/// Whether a Codex child in this mode answers its own permission prompts —
/// the fact an `autoAccept` delivery demands of the delivered mode. Two
/// routes, never a provider-name table: the daemon's broker answers the
/// provider-agnostic ids it owns (`mode_is_auto_answered`), and Codex's own
/// knob is read from the row that owns it — the `CodexMode` table's
/// `unattended: Yes` answer, the same fact the marker derives — so a new
/// `Yes` row is answered here without this predicate learning its name.
/// `full-access` is that knob today, with approval policy `never`: the
/// provider never asks anybody, so the child runs alone however the broker
/// feels. The other modes keep `on-request`, so the human may be asked and
/// a profile that ticked `autoAccept` on one is a contradiction.
fn mode_answers_own_prompts(mode_id: &str) -> bool {
    crate::provider_catalog::mode_is_auto_answered(mode_id)
        || crate::codex_view::unattended_answer(Some(mode_id))
            == devboule_protocol::UnattendedState::Yes
}

/// The tick half of [`validate_delivery`] as one predicate, shared with the
/// tests that cross it against the pre-card gate (the re-audit's P1): the
/// pre-card gate must never refuse a pair this rule accepts, and `full-access`
/// + tick is the pair that convicts — accepted here, `NotOursToJudge` there.
pub(crate) fn tick_contradicts(delivery: &ProfileDelivery) -> bool {
    let mode_id = delivery
        .mode_id
        .as_deref()
        .unwrap_or(crate::codex_view::DEFAULT_MODE);
    delivery.auto_accept && !mode_answers_own_prompts(mode_id)
}

/// The creation-time refusals Codex can make before a process exists: the
/// mode must be one of Codex's own, and an `autoAccept` tick demands a mode
/// that will not ask the human.
pub(super) fn validate_delivery(delivery: &ProfileDelivery) -> Result<(), WireError> {
    let mode_id = delivery
        .mode_id
        .as_deref()
        .unwrap_or(crate::codex_view::DEFAULT_MODE);
    validate_mode(mode_id)?;
    if tick_contradicts(delivery) {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!(
                "the profile asks Codex to approve its own permission prompts and also to start in mode '{mode_id}', which asks the human; the two contradict, so the creation is refused"
            ),
        ));
    }
    Ok(())
}

/// The model and thinking option the profile named, seeded into the state the
/// first `turn/start` reads its parameters from. The handshake's model/list
/// catalog is the vocabulary, so the refusal carries the two distinct
/// sentences: a provider that publishes no models is not a provider whose
/// named model is unknown.
fn seed_model_and_effort(
    state: &Arc<CodexState>,
    delivery: &ProfileDelivery,
) -> Result<(), WireError> {
    if delivery.model_id.is_some() || delivery.thinking_option_id.is_some() {
        let empty = state.catalog_is_empty();
        if empty && delivery.model_id.is_some() {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "the profile names a model, but Codex publishes no models the daemon can deliver; the creation is refused rather than started on a different model",
            ));
        }
        state.set_model(
            delivery.model_id.as_deref(),
            delivery.thinking_option_id.as_deref(),
        )?;
    }
    Ok(())
}

/// The env name Codex reads its home directory from. Family knowledge lives in
/// this module (rule 1); the broker's own env names (`DEVBOULE_MCP_TOKEN`,
/// `DEVBOULE_MCP_URL`) live beside the door in `mcp_broker`.
pub(crate) const CODEX_HOME_ENV: &str = "CODEX_HOME";
/// Owned per-session home dir names under the runtime dir. The sweep removes
/// whole trees by this name alone — never by content, never outside our names —
/// so a Codex child's goals, logs, sqlite and `installation_id` (probe-measured
/// per-child state) can never leak into a sibling or survive teardown.
const CODEX_HOME_PREFIX: &str = "devboule-codex-home-";
static CODEX_HOME_COUNTER: AtomicU64 = AtomicU64::new(1);

/// One MCP server entry in a Codex `config.toml`, generated from typed values
/// (S6). The table name is the broker's `MCP_SERVER_NAME`; the token travels
/// as a child-env name, never a value on disk — the secret on disk is the
/// pointer, the secret in memory is the env, the same exposure class as
/// Claude's inline-bearer file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct CodexMcpServer {
    url: String,
    bearer_token_env_var: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
struct CodexHomeConfig {
    #[serde(default)]
    mcp_servers: std::collections::BTreeMap<String, CodexMcpServer>,
}

/// Render the home's `config.toml` from typed values. `BTreeMap` so the bytes
/// are deterministic and the parse-back below reads exactly what was written.
fn render_codex_config(url: &str) -> String {
    let mut mcp_servers = std::collections::BTreeMap::new();
    mcp_servers.insert(
        crate::mcp_broker::MCP_SERVER_NAME.to_string(),
        CodexMcpServer {
            url: url.to_string(),
            bearer_token_env_var: crate::mcp_broker::MCP_TOKEN_ENV.to_string(),
        },
    );
    toml::to_string(&CodexHomeConfig { mcp_servers }).expect("typed Codex config serializes")
}

/// The parse-back before the write (S6): the bytes must parse as TOML *and*
/// carry our server entry with our URL and our token-env pointer. A file that
/// does not round-trip never reaches the child — without this, a broken writer
/// reproduces the probe's silent dead end (`Invalid configuration; using
/// defaults`, server keeps serving, no tools, nobody told).
fn parse_back_codex_config(text: &str) -> Result<CodexMcpServer, String> {
    let config: CodexHomeConfig = toml::from_str(text)
        .map_err(|error| format!("Codex home config does not parse as TOML: {error}"))?;
    config
        .mcp_servers
        .get(crate::mcp_broker::MCP_SERVER_NAME)
        .cloned()
        .filter(|server| {
            !server.url.is_empty()
                && server.bearer_token_env_var == crate::mcp_broker::MCP_TOKEN_ENV
        })
        .ok_or_else(|| {
            "Codex home config lacks the devboule MCP server entry with our URL and token pointer"
                .to_string()
        })
}

/// Parse-back plus protected write, in that order: a refusal leaves neither a
/// home dir nor a config behind. The text goes through S4's
/// `write_protected_bytes` (create_new, DACL before the first byte, sync,
/// rename) — no second recipe.
fn write_codex_home_with(home: &Path, text: &str) -> io::Result<()> {
    parse_back_codex_config(text).map_err(io::Error::other)?;
    crate::mcp_broker::write_protected_str(&home.join("config.toml"), text)
}

pub(crate) fn write_codex_home(home: &Path, url: &str) -> io::Result<()> {
    write_codex_home_with(home, &render_codex_config(url))
}

fn codex_home_path(runtime_dir: &Path) -> PathBuf {
    let serial = CODEX_HOME_COUNTER.fetch_add(1, Ordering::Relaxed);
    runtime_dir.join(format!("{CODEX_HOME_PREFIX}{serial}"))
}

fn remove_codex_home(home: &Path) {
    if let Err(error) = std::fs::remove_dir_all(home) {
        if error.kind() != io::ErrorKind::NotFound {
            eprintln!("could not remove Codex home {}: {error}", home.display());
        }
    }
}

/// The handshake assertion (S6): the `initialize` result's echoed `codexHome`
/// must be the directory chosen — canonicalised on BOTH sides, because Windows
/// symlink/case normalisation makes raw string equality refuse healthy children
/// (plan correction #3). Mismatch, absence, or an unreadable dir means the env
/// redirect did not take and the child would read the human's real `~/.codex`:
/// refuse loudly, never run against the wrong home. The one loud failure Codex
/// version drift (Q11) produces — a renamed echo key reads as absent here.
fn assert_codex_home(echoed: Option<&str>, expected_home: &Path) -> Result<(), WireError> {
    let echoed = echoed.filter(|value| !value.is_empty()).ok_or_else(|| {
        WireError::new(
            ErrorCode::Io,
            "Codex initialize response had no codexHome; the home redirect cannot be verified, so the child is refused rather than run against an unknown home.",
        )
    })?;
    let expected = expected_home.canonicalize().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not canonicalize the chosen Codex home: {error}"),
        )
    })?;
    let actual = Path::new(echoed).canonicalize().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not canonicalize the Codex home the child reports ({echoed}): {error}"),
        )
    })?;
    if actual != expected {
        return Err(WireError::new(
            ErrorCode::Io,
            format!(
                "Codex runs in {}, not the chosen home {}; refusing rather than reading the human's real config.",
                actual.display(),
                expected.display()
            ),
        ));
    }
    Ok(())
}

/// The Codex carrier seam (S4 shape, S6 body — the provider-trait signatures
/// verbatim, so adoption is a move): from the launch config the broker minted,
/// the env the child needs and the owned home. Token travels as child **env**
/// (`DEVBOULE_MCP_TOKEN`), `CODEX_HOME` joins it the same way. Never argv —
/// the argv token-free assertion in S6 tests pins this, and the URL rides the
/// config file inline (it is not a secret; the names carry no secret bytes).
pub(crate) fn mcp_launch(
    config: &crate::mcp_broker::McpLaunchConfig,
    runtime_dir: &Path,
) -> Result<crate::mcp_broker::McpProviderConfig, WireError> {
    let home = codex_home_path(runtime_dir);
    write_codex_home(&home, &config.url).map_err(|error| {
        remove_codex_home(&home);
        WireError::new(
            ErrorCode::Io,
            format!("Could not prepare the Codex home: {error}"),
        )
    })?;
    Ok(crate::mcp_broker::McpProviderConfig {
        env_additions: vec![
            (
                crate::mcp_broker::MCP_TOKEN_ENV.to_string(),
                config.bearer().to_string(),
            ),
            (
                CODEX_HOME_ENV.to_string(),
                home.to_string_lossy().into_owned(),
            ),
        ],
        arg_additions: Vec::new(),
        owned_paths: vec![home.join("config.toml")],
        owned_dirs: vec![home],
    })
}

/// Spawn Codex (S6 wiring, S9 live): `mcp` is the broker's launch config when
/// the session was registered for MCP tools, `None` otherwise. `None` is exactly
/// the old behaviour — no home dir, no extra env, no handshake assertion — and
/// stays the road for unregistered sessions. `Some` builds
/// the per-session `CODEX_HOME` (our `config.toml` naming the broker by env-var
/// pointer), joins `DEVBOULE_MCP_TOKEN` + `CODEX_HOME` onto the child env (never
/// argv), and asserts the `initialize` echo names the chosen dir — canonicalised
/// both sides — refusing loudly otherwise, never running against the human's
/// real home. A per-session home means per-session empty state (goals, sqlite,
/// `installation_id`); fleet cost disclosed in S7/Q5, not solved here.
pub(super) fn spawn_process(
    state: &Arc<ServerState>,
    command: PtyCommand,
    mcp: Option<crate::mcp_broker::McpLaunchConfig>,
    delivery: ProfileDelivery,
) -> Result<SpawnedSession, WireError> {
    validate_delivery(&delivery)?;
    let mode_id = delivery
        .mode_id
        .as_deref()
        .unwrap_or(crate::codex_view::DEFAULT_MODE)
        .to_string();
    // The carrier, only when the broker minted one (S9: registered sessions;
    // unregistered spawns keep the `None` road below, byte-identical).
    let carrier = match mcp.as_ref() {
        Some(config) => Some(mcp_launch(config, state.sessions.runtime_dir())?),
        None => None,
    };
    let codex_home: Option<PathBuf> = carrier
        .as_ref()
        .and_then(|carrier| carrier.owned_dirs.first().cloned());
    // S4 seam invariants, pinned loudly: Codex carries no verbatim argv
    // additions (its launch line is already complete from the catalog) — a
    // carrier violating that fails here, not in the child.
    if let Some(carrier) = carrier.as_ref() {
        assert!(
            carrier.arg_additions.is_empty(),
            "codex carrier is env + owned home, nothing else"
        );
    }
    let remove_home = |codex_home: &Option<PathBuf>| {
        if let Some(home) = codex_home {
            remove_codex_home(home);
        }
    };

    let mut process = Command::new(&command.program);
    process
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &command.env {
        process.env(key, value);
    }
    // The carrier env, only when the broker minted one: token + home as child
    // env (never argv — the S6 argv token-free assertion pins this).
    if let Some(carrier) = carrier.as_ref() {
        for (key, value) in &carrier.env_additions {
            process.env(key, value);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        process.creation_flags(0x0800_0000);
    }
    let mut child = process.spawn().map_err(|error| {
        remove_home(&codex_home);
        WireError::new(
            ErrorCode::Io,
            format!("Could not start Codex {}: {error}", command.program),
        )
    })?;

    #[cfg(windows)]
    let (process_job, os_handle) = {
        use std::os::windows::io::AsRawHandle;
        let process_job = JobObject::new().map_err(|error| {
            terminate_process(&mut child);
            remove_home(&codex_home);
            WireError::new(
                ErrorCode::Io,
                format!("Could not create the Codex process job: {error}"),
            )
        })?;
        let handle = child.as_raw_handle();
        if let Err(error) = state
            .process_job
            .assign(handle)
            .and_then(|()| process_job.assign(handle))
        {
            terminate_process(&mut child);
            remove_home(&codex_home);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not contain the Codex process: {error}"),
            ));
        }
        let os_handle = ProcessHandle::duplicate(handle).ok();
        (process_job, os_handle)
    };
    #[cfg(not(windows))]
    let process_job = JobObject::new().map_err(|error| {
        terminate_process(&mut child);
        remove_home(&codex_home);
        WireError::new(
            ErrorCode::Io,
            format!("Could not create the Codex process job: {error}"),
        )
    })?;
    #[cfg(not(windows))]
    let os_handle = None;

    let stdin = child.stdin.take().ok_or_else(|| {
        terminate_process(&mut child);
        remove_home(&codex_home);
        WireError::new(ErrorCode::Io, "Codex did not provide stdin.")
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        terminate_process(&mut child);
        remove_home(&codex_home);
        WireError::new(ErrorCode::Io, "Codex did not provide stdout.")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        terminate_process(&mut child);
        remove_home(&codex_home);
        WireError::new(ErrorCode::Io, "Codex did not provide stderr.")
    })?;
    let process = Arc::new(Mutex::new(child));
    let stdin = Arc::new(Mutex::new(Some(stdin)));
    let next_id = Arc::new(AtomicU64::new(1));
    let mut stdout = CodexStdout::spawn(stdout).map_err(|error| {
        terminate_shared_process(&process);
        remove_home(&codex_home);
        WireError::new(
            ErrorCode::Io,
            format!("Could not read Codex stdout: {error}"),
        )
    })?;
    let handshake = perform_handshake(
        &mut stdout,
        &stdin,
        &next_id,
        &command.cwd,
        &mode_id,
        codex_home.as_deref(),
    )
    .inspect_err(|_| {
        terminate_shared_process(&process);
        remove_home(&codex_home);
    })?;

    let state = Arc::new(CodexState::new(
        handshake.thread_id,
        handshake.catalog,
        &mode_id,
    ));
    // The model and thinking option the profile named, delivered through the
    // state the first turn reads — and judged against the catalog the
    // handshake just brought back, so an undeliverable choice refuses here,
    // before the child is a session, instead of running something else.
    if let Err(error) = seed_model_and_effort(&state, &delivery) {
        terminate_shared_process(&process);
        remove_home(&codex_home);
        return Err(error);
    }
    let peer_session_id = state.thread_id();
    // One registration table for the requests this client awaits answers to
    // (A2-03), shared by the steerer that registers and the reader that
    // delivers.
    let requests = Arc::new(CodexRequests::new());
    // S8 trigger bundle, cloned handles only (never the reader — see
    // `CodexVerifyBundle`): present exactly when a carrier was installed, so
    // the `None` road — today's only road — verifies nothing and changes nothing.
    let pending_codex_verify = codex_verify_bundle_for(&carrier, &stdin, &next_id, &requests);
    let switcher = CodexSwitcher {
        stdin: Arc::clone(&stdin),
        next_id: Arc::clone(&next_id),
        state: Arc::clone(&state),
        requests: Arc::clone(&requests),
    };
    let response_ids = Arc::new(Mutex::new(HashMap::new()));
    let permission_broker = PermissionBroker::with_sender(codex_permission_sender(
        Arc::clone(&stdin),
        Arc::clone(&response_ids),
    ));
    let writer = CodexWriter {
        stdin: Arc::clone(&stdin),
        next_id: Arc::clone(&next_id),
        state: Arc::clone(&state),
        pending: Vec::new(),
    };
    // The static prompt route sends Codex's own `turn/start`: it shares the
    // stdin, the request-id counter and the thread state with the writer.
    let static_prompt = Arc::new(CodexStaticPrompt::new(
        Arc::clone(&stdin),
        Arc::clone(&next_id),
        Arc::clone(&state),
    ));
    let killer = CodexKiller {
        process: Arc::clone(&process),
        stdin: Arc::clone(&stdin),
        next_id: Arc::clone(&next_id),
        state: Arc::clone(&state),
        permission_broker: Arc::clone(&permission_broker),
        cancelled: Arc::new(AtomicBool::new(false)),
        codex_home: codex_home.clone(),
    };
    let reader = CodexReader {
        buffer: Vec::new(),
        discarding_oversized_line: false,
        deferred: handshake.deferred,
        manifest: Some(state.manifest()),
        state,
        view: crate::codex_view::CodexView::new(Some(command.cwd)),
        permission_broker: Arc::clone(&permission_broker),
        response_ids,
        stdin: Arc::clone(&stdin),
        next_id,
        requests,
    };
    Ok(SpawnedSession {
        process_job,
        master: None,
        killer: Box::new(killer),
        switcher: Some(Box::new(switcher)),
        child: Box::new(StdioWaitableChild { process }),
        writer: Arc::new(Mutex::new(Box::new(writer) as Box<dyn Write + Send>)),
        // Not an ACP session: no negotiated structured route. The static one
        // sends Codex's own `turn/start` frame.
        image_sink: None,
        static_image_sink: Some(static_prompt),
        reader: Box::new(stdout),
        reader_dispatch: Some(Box::new(reader)),
        stderr: Some(Box::new(CodexStderr::start(stderr))),
        permission_broker: Some(permission_broker),
        os_handle,
        peer_session_id: Some(peer_session_id),
        agent_version: None,
        // The delivery was applied inside `spawn_process`, before this value
        // existed; nothing is left for the session reader to answer.
        pending_delivery: None,
        pending_codex_verify,
    })
}

fn terminate_process(process: &mut Child) {
    let _ = process.kill();
    let _ = process.wait();
}

fn terminate_shared_process(process: &Arc<Mutex<Child>>) {
    if let Ok(mut process) = process.lock() {
        terminate_process(&mut process);
    }
}

struct CodexSwitcher {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    state: Arc<CodexState>,
    requests: Arc<CodexRequests>,
}

struct CodexSteerer {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    state: Arc<CodexState>,
    requests: Arc<CodexRequests>,
}

impl SessionSteerer for CodexSteerer {
    fn steer_active_turn(
        &mut self,
        text: &str,
        turn: &mut TurnToken<'_>,
    ) -> Result<bool, WireError> {
        // The turn to steer is read here and checked again at the moment of
        // writing, inside `begin_steer`: a steer written for a turn Codex has
        // already left is not written at all. The daemon's admission token is
        // what keeps this side honest — it is held across this write, so the
        // runtime's turn cannot end between the caller's check and this frame
        // (S4-02) — while the id Codex compares against is its own turn id,
        // which is the only thing its protocol understands (S4-01 on this
        // provider).
        let Some(turn_id) = self.state.current_turn() else {
            return Ok(false);
        };
        // The write stays under the token's hold; the *answer* does not
        // (A2-03). A `turn/steer` is a request: writing it is not Codex taking
        // it, and the response is what says whether it did. That response is
        // delivered by the reader thread, which also publishes the events that
        // take this runtime's turn-hold — so waiting for it under the hold
        // would block the very thread that has to deliver it, exactly as it
        // would for Pi. The hold therefore ends with the write, and the wait
        // runs outside it: `Ok(true)` is only ever answered from a response
        // that names this turn.
        let request = turn.write_then_release(|| self.begin_steer(&turn_id, text))?;
        let Some((command_id, response)) = request else {
            return Ok(false);
        };
        self.await_steer_response(&command_id, &turn_id, response)
    }

    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(Self {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            state: Arc::clone(&self.state),
            requests: Arc::clone(&self.requests),
        })
    }
}

impl CodexSteerer {
    /// Register the answer channel and write one `turn/steer` for
    /// `expected_turn_id`, answering the id its response will name.
    ///
    /// `Ok(None)` with nothing written: Codex is no longer on the turn this
    /// steer was admitted for.
    fn begin_steer(
        &self,
        expected_turn_id: &str,
        text: &str,
    ) -> Result<Option<(String, RequestAnswer)>, WireError> {
        let Some(params) = steer_params_if_current(&self.state, expected_turn_id, text) else {
            return Ok(None);
        };
        let id = format!("d-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let Some(response) = self.requests.register(&id) else {
            return Err(WireError::new(
                ErrorCode::Io,
                "Codex request map is unavailable.",
            ));
        };
        if let Err(error) = send_frame(
            &self.stdin,
            &request_frame(&id, "turn/steer", params),
            "Codex",
        ) {
            self.requests.forget(&id);
            return Err(error);
        }
        Ok(Some((id, response)))
    }

    /// Wait for the response to one `turn/steer` and answer whether Codex took
    /// the steer for the turn it was written for.
    ///
    /// An error result, a response naming another turn, and a response with no
    /// turn at all are all `Ok(false)`: none of them is evidence that the text
    /// landed in this turn, so the caller's pre-existing fallback is the honest
    /// answer, never `Ok(true)`. A timeout is an `Err` — the steer's fate is
    /// then unknown, which is a different thing from knowing it did not land.
    fn await_steer_response(
        &self,
        command_id: &str,
        expected_turn_id: &str,
        response: RequestAnswer,
    ) -> Result<bool, WireError> {
        let value = response
            .recv_timeout(STEER_RESPONSE_TIMEOUT)
            .map_err(|error| {
                self.requests.forget(command_id);
                WireError::new(
                    ErrorCode::Io,
                    format!("Codex turn/steer response timed out: {error}"),
                )
            })?
            // The transport's own answer: the channel ended before a response
            // came (S4-04), so the steer's fate is unknown — an `Err`, not a
            // refusal, exactly as Pi answers one.
            .map_err(|message| WireError::new(ErrorCode::Io, message))?;
        Ok(steer_response_accepted(&value, expected_turn_id))
    }
}

/// Whether one `turn/steer` response says Codex took the steer for
/// `expected_turn_id` (A2-03).
///
/// The protocol's own confirmation is the turn the response reports: an error
/// result means the request was refused, and a response for a different turn (or
/// without one) is not evidence about *this* turn. Only a matching turn id is
/// `true`.
fn steer_response_accepted(value: &Value, expected_turn_id: &str) -> bool {
    if value.get("error").is_some() {
        return false;
    }
    turn_id_from_response(value).as_deref() == Some(expected_turn_id)
}

impl ModelSwitcher for CodexSwitcher {
    fn set_model(&self, model_id: Option<&str>, effort: Option<&str>) -> Result<(), WireError> {
        self.state.set_model(model_id, effort)
    }

    fn set_mode(&self, mode_id: &str) -> Result<(), WireError> {
        self.state.set_mode(mode_id)
    }

    fn manifest(&self) -> Option<SessionEvent> {
        Some(self.state.manifest())
    }

    fn clone_switcher(&self) -> Box<dyn ModelSwitcher> {
        Box::new(Self {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            state: Arc::clone(&self.state),
            requests: Arc::clone(&self.requests),
        })
    }

    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(CodexSteerer {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            state: Arc::clone(&self.state),
            requests: Arc::clone(&self.requests),
        })
    }
}

/// The static prompt route for Codex: it plans, then sends Codex's own
/// `turn/start` — the frame this provider's protocol defines for a prompt that
/// carries images.
pub(crate) struct CodexStaticPrompt {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    state: Arc<CodexState>,
}

impl CodexStaticPrompt {
    fn new(
        stdin: Arc<Mutex<Option<ChildStdin>>>,
        next_id: Arc<AtomicU64>,
        state: Arc<CodexState>,
    ) -> Self {
        Self {
            stdin,
            next_id,
            state,
        }
    }
}

impl super::StaticImageSink for CodexStaticPrompt {
    fn plan_prompt(
        &self,
        store: &AttachmentStore,
        session_id: &str,
        text: &str,
        attachments: &[devboule_protocol::PromptAttachment],
    ) -> Result<Option<Box<dyn super::PlannedStaticPrompt>>, WireError> {
        let Some(plan) = plan_codex_prompt(store, session_id, text, attachments)? else {
            return Ok(None);
        };
        Ok(Some(Box::new(CodexPlannedPrompt {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            state: Arc::clone(&self.state),
            plan,
        })))
    }
}

/// One planned Codex prompt, ready to send. It carries the plan whole, so the
/// text on the wire and the text the caller journals cannot be two different
/// strings.
struct CodexPlannedPrompt {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    state: Arc<CodexState>,
    plan: CodexPromptPlan,
}

impl CodexPlannedPrompt {
    /// The `turn/start` params this prompt sends: the model, effort and policy
    /// override are read at send time, the way the writer reads them, so a
    /// model switched between prompt and send is not sent a stale name.
    fn params(&self) -> Value {
        turn_start_params_for_prompt(
            &self.state,
            &self.plan.fallback_text,
            &self.plan.image_paths,
        )
    }
}

impl super::PlannedStaticPrompt for CodexPlannedPrompt {
    fn text(&self) -> &str {
        &self.plan.fallback_text
    }

    /// The references join this plan's text as path lines, after the fallback
    /// lines its own attachments left there and never as `localImage` paths:
    /// see `session::push_reference_path_lines`.
    fn append_reference_path_lines(&mut self, reference_paths: &[std::path::PathBuf]) {
        super::push_reference_path_lines(&mut self.plan.fallback_text, reference_paths);
    }

    fn send(&self) -> Result<(), WireError> {
        send_request(
            &self.stdin,
            &self.next_id,
            "turn/start",
            self.params(),
            "Codex",
        )
    }
}

struct CodexWriter {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    state: Arc<CodexState>,
    pending: Vec<u8>,
}

impl Write for CodexWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        // The text-only `turn/start`, unchanged. A prompt that carries
        // images is sent by the static route's own `turn/start` instead of by
        // this writer, so `turn_start_params_with_images` has one production
        // caller and this literal keeps the other: with no carried path the
        // two build the same params, which
        // `a_params_builder_without_images_is_the_text_only_one` pins.
        let (model, effort) = self.state.model_and_effort();
        let policy_mode = self.state.mode_override();
        send_request(
            &self.stdin,
            &self.next_id,
            "turn/start",
            turn_start_params(
                &self.state.thread_id(),
                &text,
                policy_mode.as_deref(),
                Some(&model),
                effort.as_deref(),
            ),
            "Codex",
        )
        .map_err(wire_to_io)
    }
}

/// How long one verification poll may wait for its answer before the outcome
/// is "not established" (S7). Above the probe-measured 7.4 s failure block
/// with margin; the startup never waits for it (detached thread, S8 trigger).
const VERIFY_TIMEOUT: Duration = Duration::from_secs(20);

/// What one `mcpServerStatus/list` result establishes (S7 → S8 tri-state).
///
/// Read the CHILD's protocol answer — the in-child round-trip (Q10) — never the
/// model-facing tools array: no verification input here is that array, by
/// construction (the signatures take stdin/requests/payload only). Measured on
/// codex-cli 0.154.0: `runtimeStatus` is null working AND failing, so it is
/// keyed on nothing; presence + catalog + null error is the working correlate.
fn map_codex_status(result: &Value) -> crate::mcp_broker::ToolsState {
    use crate::mcp_broker::ToolsState;
    let ours = result
        .get("data")
        .and_then(Value::as_array)
        .and_then(|data| {
            data.iter().find(|entry| {
                entry.get("name").and_then(Value::as_str)
                    == Some(crate::mcp_broker::MCP_SERVER_NAME)
            })
        });
    let Some(ours) = ours else {
        // Never configured, or vanished: not proof of absence.
        return ToolsState::Unverified;
    };
    let failed = ours
        .get("toolsError")
        .and_then(Value::as_str)
        .is_some_and(|message| !message.is_empty());
    if failed {
        // Configured and failed: established, without working tools.
        return ToolsState::Unverified;
    }
    let catalogued = ours
        .get("tools")
        .and_then(Value::as_object)
        .is_some_and(|tools| !tools.is_empty());
    if catalogued {
        ToolsState::Hosted
    } else {
        // No error but no catalog either: no evidence either way.
        ToolsState::Unverified
    }
}

/// The carrier observations, handed to the detached verification thread (S8
/// trigger). Cloned handles only — stdin, id counter, id-matched requests —
/// never the reader: a synchronous round-trip inside an event callback would
/// park the deliverer (attached-connection-loses-events reentrancy rule), so
/// the signature cannot express that placement. Built only with a carrier.
#[derive(Clone)]
pub(crate) struct CodexVerifyBundle {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    requests: Arc<CodexRequests>,
}

/// One bounded verification poll, from any non-reader context (S7).
/// Authoritative, bounded, degrading: every outcome but the golden maps to
/// `Unverified` — a timeout proves nothing, a transport end wakes via
/// `fail_pending`, a refusal or malformed answer is not evidence. Never
/// `Unavailable`: verification cannot prove absence (that is the no-carrier
/// spawn fact, decided where the carrier is or is not built).
///
/// The startup never waits for this (S8 runs it detached); the first prompt
/// proceeds whatever it answers.
pub(crate) fn verify_codex_mcp(bundle: &CodexVerifyBundle) -> crate::mcp_broker::ToolsState {
    verify_codex_mcp_with_timeout(bundle, VERIFY_TIMEOUT)
}

fn verify_codex_mcp_with_timeout(
    bundle: &CodexVerifyBundle,
    timeout: Duration,
) -> crate::mcp_broker::ToolsState {
    use crate::mcp_broker::ToolsState;
    let id = format!("d-{}", bundle.next_id.fetch_add(1, Ordering::Relaxed));
    let Some(answer) = bundle.requests.register(&id) else {
        return ToolsState::Unverified;
    };
    if send_frame(
        &bundle.stdin,
        &request_frame(&id, "mcpServerStatus/list", serde_json::json!({})),
        "Codex",
    )
    .is_err()
    {
        bundle.requests.forget(&id);
        return ToolsState::Unverified;
    }
    let answer = answer.recv_timeout(timeout);
    match answer {
        Ok(Ok(value)) => {
            if value.get("error").is_some() {
                return ToolsState::Unverified;
            }
            map_codex_status(value.get("result").unwrap_or(&Value::Null))
        }
        Ok(Err(_)) => {
            // The transport ended (`fail_pending` drained us): no answer can
            // come. Unverified, and fast — never a hung turn.
            ToolsState::Unverified
        }
        Err(_) => {
            bundle.requests.forget(&id);
            ToolsState::Unverified
        }
    }
}

/// Which spawns verify: exactly the spawns that installed a carrier (S8). One
/// function so the test pins the rule without spawning anything.
pub(crate) fn codex_verify_bundle_for(
    carrier: &Option<crate::mcp_broker::McpProviderConfig>,
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    next_id: &Arc<AtomicU64>,
    requests: &Arc<CodexRequests>,
) -> Option<CodexVerifyBundle> {
    carrier.as_ref().map(|_| CodexVerifyBundle {
        stdin: Arc::clone(stdin),
        next_id: Arc::clone(next_id),
        requests: Arc::clone(requests),
    })
}

struct CodexKiller {
    process: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    state: Arc<CodexState>,
    permission_broker: Arc<PermissionBroker>,
    cancelled: Arc<AtomicBool>,
    codex_home: Option<PathBuf>,
}

impl SessionKiller for CodexKiller {
    fn interrupt(&mut self) {
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        let _ = send_interrupt_request(&self.stdin, &self.next_id, &self.state);
        self.permission_broker.cancel_pending();
    }

    fn kill(&mut self) {
        if self.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        self.permission_broker.close();
        if let Ok(mut stdin) = self.stdin.lock() {
            *stdin = None;
        }
        // Grace for a natural exit first — but the home is removed on EVERY
        // path below, never just the kill path: the old early `return` on a
        // reaped child skipped the removal and leaked the whole tree (S9 road
        // test caught it: a child that exits on stdin close left its home).
        let deadline = Instant::now() + KILL_GRACE;
        while Instant::now() < deadline {
            if self
                .process
                .lock()
                .ok()
                .and_then(|mut process| process.try_wait().ok())
                .flatten()
                .is_some()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Ok(mut process) = self.process.lock() {
            if process.try_wait().ok().flatten().is_none() {
                let _ = process.kill();
                // Bounded re-wait so the home goes away only once nobody can
                // use it; the teardown's job kill follows within milliseconds.
                let deadline = Instant::now() + Duration::from_secs(2);
                while Instant::now() < deadline {
                    if process.try_wait().ok().flatten().is_some() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        if let Some(home) = self.codex_home.as_deref() {
            remove_codex_home(home);
        }
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self {
            process: Arc::clone(&self.process),
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            state: Arc::clone(&self.state),
            permission_broker: Arc::clone(&self.permission_broker),
            cancelled: Arc::clone(&self.cancelled),
            codex_home: self.codex_home.clone(),
        })
    }
}

struct Handshake {
    thread_id: String,
    catalog: CodexCatalog,
    deferred: Vec<Value>,
}

fn perform_handshake(
    stdout: &mut CodexStdout,
    stdin: &Mutex<Option<ChildStdin>>,
    next_id: &AtomicU64,
    cwd: &Path,
    mode_id: &str,
    expected_home: Option<&Path>,
) -> Result<Handshake, WireError> {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut deferred = Vec::new();
    let initialize = request_response(
        stdout,
        stdin,
        next_id,
        "initialize",
        initialize_params(),
        deadline,
        &mut deferred,
    )?;
    // S6: the env redirect either took or the child reads the human's home.
    // `None` (today's road) skips the check; `Some` refuses loudly on mismatch.
    if let Some(expected) = expected_home {
        assert_codex_home(
            initialize.get("codexHome").and_then(Value::as_str),
            expected,
        )?;
    }
    send_frame(
        stdin,
        &notification_frame("initialized", serde_json::json!({})),
        "Codex",
    )?;
    let models_response = request_response(
        stdout,
        stdin,
        next_id,
        "model/list",
        serde_json::json!({}),
        deadline,
        &mut deferred,
    )?;
    let mut catalog = catalog_from_response(&models_response)?;
    let thread_response = request_response(
        stdout,
        stdin,
        next_id,
        "thread/start",
        thread_start_params(cwd, mode_id),
        deadline,
        &mut deferred,
    )?;
    let thread_id = thread_response
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| handshake_error("thread/start response had no thread.id"))?
        .to_string();
    catalog.apply_thread_response(&thread_response);
    Ok(Handshake {
        thread_id,
        catalog,
        deferred,
    })
}

fn initialize_params() -> Value {
    // Paseo's non-originating identity: Codex keeps its own CLI identity in
    // provider usage logs instead of showing the daemon as the originator.
    serde_json::json!({
        "clientInfo": {
            "name": "codex_app_server_daemon",
            "title": "Codex App Server Daemon",
            "version": "0.0.0",
        },
        "capabilities": {
            "experimentalApi": true,
            "mcpServerOpenaiFormElicitation": true,
        },
    })
}

fn request_response(
    stdout: &mut CodexStdout,
    stdin: &Mutex<Option<ChildStdin>>,
    next_id: &AtomicU64,
    method: &str,
    params: Value,
    deadline: Instant,
    deferred: &mut Vec<Value>,
) -> Result<Value, WireError> {
    let id = format!("d-{}", next_id.fetch_add(1, Ordering::Relaxed));
    send_frame(stdin, &request_frame(&id, method, params), "Codex")?;
    loop {
        let line = stdout
            .next_line(deadline)
            .map_err(|error| {
                handshake_error(&format!("could not read {method} response: {error}"))
            })?
            .ok_or_else(|| handshake_error(&format!("Codex exited before {method} completed")))?;
        let value: Value = serde_json::from_str(&line)
            .map_err(|error| handshake_error(&format!("malformed {method} response: {error}")))?;
        if value.get("id").and_then(Value::as_str) == Some(id.as_str()) {
            if let Some(error) = value.get("error") {
                return Err(handshake_error(
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("request failed"),
                ));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
        deferred.push(value);
    }
}

fn send_request(
    stdin: &Mutex<Option<ChildStdin>>,
    next_id: &AtomicU64,
    method: &str,
    params: Value,
    label: &'static str,
) -> Result<(), WireError> {
    let id = format!("d-{}", next_id.fetch_add(1, Ordering::Relaxed));
    send_frame(stdin, &request_frame(&id, method, params), label)
}

fn request_frame(id: &str, method: &str, params: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn method_not_supported_frame(id: &Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32601,
            "message": "method not supported",
        },
    })
}

fn send_interrupt_request(
    stdin: &Mutex<Option<ChildStdin>>,
    next_id: &AtomicU64,
    state: &CodexState,
) -> Result<bool, WireError> {
    let Some(turn_id) = state.current_turn() else {
        eprintln!("Codex interrupt ignored: no current turn.");
        return Ok(false);
    };
    send_request(
        stdin,
        next_id,
        "turn/interrupt",
        interrupt_params(&state.thread_id(), &turn_id),
        "Codex",
    )
    .map(|()| true)
}

fn turn_id_from_response(value: &Value) -> Option<String> {
    value
        .pointer("/result/turn/id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn notification_frame(method: &str, params: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

fn turn_start_params(
    thread_id: &str,
    text: &str,
    policy_mode: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Value {
    // The thread already carries the mode preset from `thread/start`; the
    // policy fields go back only after an explicit `set_mode`.
    let mut params = policy_mode.map(mode_values).unwrap_or_default();
    params.insert("threadId".to_string(), Value::String(thread_id.to_string()));
    params.insert(
        "input".to_string(),
        serde_json::json!([{ "type": "text", "text": text }]),
    );
    if let Some(model) = model {
        params.insert("model".to_string(), Value::String(model.to_string()));
    }
    if let Some(effort) = effort {
        params.insert("effort".to_string(), Value::String(effort.to_string()));
    }
    Value::Object(params)
}

fn turn_steer_params(thread_id: &str, expected_turn_id: &str, text: &str) -> Value {
    serde_json::json!({
        "threadId": thread_id,
        "expectedTurnId": expected_turn_id,
        "input": [{"type": "text", "text": text}],
    })
}

/// The `turn/steer` params for the turn the caller captured, or `None` when
/// Codex is no longer running that turn.
///
/// This is the write-time check: `current_turn()` is read again here, after the
/// caller's own capture, so a turn that ended (or a new one that started) in
/// between makes the steer unavailable instead of sending Codex a frame for a
/// turn that is over. `expectedTurnId` carries that same id, which is also the
/// precondition Codex itself checks.
fn steer_params_if_current(
    state: &CodexState,
    expected_turn_id: &str,
    text: &str,
) -> Option<Value> {
    (state.current_turn().as_deref() == Some(expected_turn_id))
        .then(|| turn_steer_params(&state.thread_id(), expected_turn_id, text))
}

/// One `turn/start` input entry for a materialized raster: the Paseo-measured
/// `{"type": "localImage", "path": ...}` shape. The path is the one
/// `materialize` returned, so the file at the other end is still the stripped
/// file — no bytes travel. `detail` is omitted deliberately: `auto` is the
/// server's own default and sending it would assert an unmeasured choice.
/// Stays on `localImage` even though the schema also lists `image`: Paseo
/// sends `localImage` and that is what was verified live, while `image` on
/// this surface has not been measured.
fn codex_local_image_entry(path: &std::path::Path) -> serde_json::Value {
    serde_json::json!({
        "type": "localImage",
        "path": path.to_string_lossy(),
    })
}

/// `turn/start` params with one trailing `localImage` entry per carried
/// raster, in attachment order, after the single text entry. The text entry
/// is the shared fallback text: the user's text plus the path lines for the
/// attachments that stay prose (SVG, which takes no inline shape).
///
/// With no carried path it builds exactly what `turn_start_params` builds —
/// `a_params_builder_without_images_is_the_text_only_one` pins that — which is
/// what lets the static route send every prompt it plans through this one
/// builder.
fn turn_start_params_with_images(
    thread_id: &str,
    text: &str,
    image_paths: &[std::path::PathBuf],
    policy_mode: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Value {
    // The thread already carries the mode preset from `thread/start`; the
    // policy fields go back only after an explicit `set_mode`.
    let mut params = policy_mode.map(mode_values).unwrap_or_default();
    params.insert("threadId".to_string(), Value::String(thread_id.to_string()));
    // The text entry first, then one `localImage` entry per carried raster,
    // in attachment order.
    let mut input = vec![serde_json::json!({ "type": "text", "text": text })];
    input.extend(image_paths.iter().map(|path| codex_local_image_entry(path)));
    params.insert("input".to_string(), Value::Array(input));
    if let Some(model) = model {
        params.insert("model".to_string(), Value::String(model.to_string()));
    }
    if let Some(effort) = effort {
        params.insert("effort".to_string(), Value::String(effort.to_string()));
    }
    Value::Object(params)
}

/// The `turn/start` params for one planned prompt: the model, effort and policy
/// override are read here, at send time, the way the writer reads them, so a
/// model switched between planning and sending is never sent a stale name.
fn turn_start_params_for_prompt(
    state: &CodexState,
    text: &str,
    image_paths: &[std::path::PathBuf],
) -> Value {
    let (model, effort) = state.model_and_effort();
    let policy_mode = state.mode_override();
    turn_start_params_with_images(
        &state.thread_id(),
        text,
        image_paths,
        policy_mode.as_deref(),
        Some(&model),
        effort.as_deref(),
    )
}

/// Prompt plan for one Codex send: the text plus one `localImage` path per
/// raster. `fallback_text` is the user's text with the path lines for the
/// non-raster attachments (SVG). `image_paths` are the `materialize` paths
/// for the rasters, in attachment order — every attachment is materialized
/// first, exactly the call the shared `with_attachment_paths` makes, so a
/// request that fails on its third attachment leaves nothing half-built.
///
/// It is handed to `CodexStaticPrompt`, which sends it as Codex's own
/// `turn/start`.
struct CodexPromptPlan {
    fallback_text: String,
    image_paths: Vec<std::path::PathBuf>,
}

/// The delivery Codex is authorised for: a fact about the protocol, not a
/// fact the peer agreed to — no negotiation, no capability probe. There is
/// deliberately no probe here: an unknown method on this surface answers
/// `-32600`, not `-32601`, so a method-not-found fallback would never fire and
/// the feature would fail silent.
fn codex_delivery() -> super::ImageDelivery {
    super::ImageDelivery::StaticImageBlock
}

/// Splits one request's attachments into `localImage` paths and path-line
/// fallbacks, each attachment materialized exactly once — the call the shared
/// `with_attachment_paths` makes.
///
/// `None` means the route did not run at all: no attachments, or a delivery
/// this sender is not authorised for. When it does run it answers with the
/// text as well, even if no raster became a `localImage` path, so that the
/// caller never has to walk the attachments a second time; with no carried
/// path the frame is the text-only `turn/start`, byte for byte.
fn plan_codex_prompt(
    store: &AttachmentStore,
    session_id: &str,
    text: &str,
    attachments: &[devboule_protocol::PromptAttachment],
) -> Result<Option<CodexPromptPlan>, devboule_protocol::WireError> {
    if attachments.is_empty() {
        return Ok(None);
    }
    // The static gate, read through the shared enum so a later change to
    // what "statically known" authorises cannot silently re-route this
    // sender: only the reserved variant frames paths.
    if codex_delivery() != super::ImageDelivery::StaticImageBlock {
        return Ok(None);
    }
    let session = store.session(session_id).ok_or_else(|| {
        devboule_protocol::WireError::new(
            devboule_protocol::ErrorCode::InvalidRequest,
            "Invalid session id.",
        )
    })?;
    let mut image_paths = Vec::new();
    let mut fallback_paths = Vec::new();
    for attachment in attachments {
        let path = session.materialize(attachment)?;
        if crate::raster_metadata::RasterMime::from_mime_type(&attachment.mime_type).is_some() {
            image_paths.push(path);
        } else {
            fallback_paths.push(path);
        }
    }
    Ok(Some(CodexPromptPlan {
        fallback_text: super::prompt_text_with_fallback_paths(text, &fallback_paths),
        image_paths,
    }))
}

/// The read-side twin of the routing rule above: what the daemon kept on
/// disk for a carried entry. Used by tests to pin the rule without spawning
/// a child.
#[cfg(test)]
fn carried_image_paths(plan: Option<&CodexPromptPlan>) -> Vec<&std::path::Path> {
    plan.map(|plan| {
        plan.image_paths
            .iter()
            .map(std::path::PathBuf::as_path)
            .collect()
    })
    .unwrap_or_default()
}

fn interrupt_params(thread_id: &str, turn_id: &str) -> Value {
    serde_json::json!({ "threadId": thread_id, "turnId": turn_id })
}

fn thread_start_params(cwd: &Path, mode_id: &str) -> Value {
    let mut params = thread_mode_values(mode_id);
    params.insert("model".to_string(), Value::Null);
    params.insert(
        "cwd".to_string(),
        Value::String(cwd.to_string_lossy().into_owned()),
    );
    Value::Object(params)
}

fn send_frame(
    stdin: &Mutex<Option<ChildStdin>>,
    frame: &Value,
    label: &'static str,
) -> Result<(), WireError> {
    let mut bytes = serde_json::to_vec(frame).map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not encode {label} frame: {error}"),
        )
    })?;
    bytes.push(b'\n');
    write_child_stdin(stdin, &bytes, label).map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not write {label} frame: {error}"),
        )
    })
}

fn wire_to_io(error: WireError) -> io::Error {
    io::Error::other(error.message)
}

fn handshake_error(message: &str) -> WireError {
    WireError::new(
        ErrorCode::Io,
        format!("Could not start Codex session: {message}"),
    )
}

fn codex_permission_sender(
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    response_ids: Arc<Mutex<HashMap<u64, Value>>>,
) -> Arc<PermissionSender> {
    Arc::new(move |broker_id, result| {
        let request_id = response_ids
            .lock()
            .map_err(|_| io::Error::other("Codex permission map lock poisoned"))?
            .get(&broker_id)
            .cloned()
            .ok_or_else(|| io::Error::other("Codex permission response had no matching request"))?;
        let frame = permission_decision_frame(&request_id, permission_decision(&result));
        let result =
            send_frame(&stdin, &frame, "Codex").map_err(|error| io::Error::other(error.message));
        if result.is_ok() {
            let _ = response_ids.lock().map(|mut ids| ids.remove(&broker_id));
        }
        result
    })
}

fn permission_decision(result: &Value) -> &'static str {
    if result.pointer("/outcome/outcome").and_then(Value::as_str) != Some("selected") {
        return "cancel";
    }
    match result.pointer("/outcome/optionId").and_then(Value::as_str) {
        Some("allow") => "accept",
        Some("deny") => "decline",
        _ => "cancel",
    }
}

struct CodexReader {
    buffer: Vec<u8>,
    discarding_oversized_line: bool,
    deferred: Vec<Value>,
    manifest: Option<SessionEvent>,
    state: Arc<CodexState>,
    view: crate::codex_view::CodexView,
    permission_broker: Arc<PermissionBroker>,
    response_ids: Arc<Mutex<HashMap<u64, Value>>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    requests: Arc<CodexRequests>,
}

impl CodexReader {
    fn publish(&self, runtime: &SessionRuntime, event: SessionEvent, seq: Option<u64>) {
        let _ = runtime.publish_agent_event_with_seq(event, None, seq);
    }

    fn dispatch_value(&mut self, value: Value, runtime: &Arc<SessionRuntime>) {
        let event_seq = runtime.journal_agent_envelope(&value);
        if value.get("method").and_then(Value::as_str)
            == Some("item/commandExecution/requestApproval")
            || value.get("method").and_then(Value::as_str)
                == Some("item/fileChange/requestApproval")
        {
            self.dispatch_permission(&value, runtime, event_seq);
            return;
        }
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            if method == "item/tool/requestUserInput" || method == "mcpServer/elicitation/request" {
                self.decline_input_request(&value, runtime);
                return;
            }
            if method == "turn/started" {
                self.state.set_turn(
                    value
                        .pointer("/params/turn/id")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                );
            } else if let Some(id) = value.get("id") {
                let _ = send_frame(&self.stdin, &method_not_supported_frame(id), "Codex");
            }
        } else if value.get("id").is_some() {
            // A response to one of this client's own requests: hand it to the
            // waiter that registered that id before anything else looks at it
            // (A2-03). A response whose id matches no waiter is ignored.
            self.requests.deliver(&value);
            if let Some(turn_id) = turn_id_from_response(&value) {
                self.state.set_turn(Some(turn_id));
            }
            if let Some(error) = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
            {
                let _ = runtime.publish_session_notice(error.to_string(), NoticeSeverity::Warning);
            }
        }
        let mut seq = event_seq;
        for event in self.view.ingest(&value) {
            if matches!(
                event,
                SessionEvent::AgentFinished { .. }
                    | SessionEvent::AgentError { .. }
                    | SessionEvent::SessionNotice { .. }
            ) && value.get("method").and_then(Value::as_str) == Some("turn/completed")
            {
                self.state.set_turn(None);
            }
            self.publish(runtime, event, seq.take());
        }
        if let Some(context_window) = self.view.take_context_window_update() {
            self.state.set_context_window(context_window);
            let manifest = runtime.store_session_manifest(self.state.manifest());
            self.publish(runtime, manifest, seq.take().or(event_seq));
        }
    }

    fn dispatch_permission(
        &mut self,
        value: &Value,
        runtime: &Arc<SessionRuntime>,
        seq: Option<u64>,
    ) {
        let params = value.get("params").unwrap_or(&Value::Null);
        if params.get("itemId").and_then(Value::as_str).is_none() {
            return;
        }
        let method = value
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let file_change = method == "item/fileChange/requestApproval";
        let Some(event) = crate::codex_view::permission_request_event(params, file_change) else {
            return;
        };
        let broker_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut ids) = self.response_ids.lock() {
            if let Some(id) = value.get("id") {
                ids.insert(broker_id, id.clone());
            }
        }
        if let Err(error) = self
            .permission_broker
            .register(broker_id, event.clone(), runtime)
        {
            self.response_ids
                .lock()
                .ok()
                .map(|mut ids| ids.remove(&broker_id));
            let _ = send_permission_decision(&self.stdin, value.get("id"), "cancel");
            let _ = runtime.publish_session_notice(
                format!("Could not queue Codex permission request: {error}"),
                NoticeSeverity::Warning,
            );
            return;
        }
        self.publish(runtime, event, seq);
    }

    fn decline_input_request(&self, value: &Value, runtime: &Arc<SessionRuntime>) {
        let result = decline_input_result(value);
        if let Some(id) = value.get("id") {
            let _ = send_result(&self.stdin, id, result);
        }
        let _ = runtime.publish_session_notice(
            "Codex requested user input; Devboule declined it.".to_string(),
            NoticeSeverity::Info,
        );
    }
}

fn decline_input_result(value: &Value) -> Value {
    if value.get("method").and_then(Value::as_str) == Some("mcpServer/elicitation/request") {
        serde_json::json!({ "action": "decline" })
    } else {
        serde_json::json!({ "answers": {} })
    }
}

impl ReaderDispatch for CodexReader {
    fn feed(&mut self, bytes: &[u8], runtime: &Arc<SessionRuntime>) -> Result<(), String> {
        if let Some(manifest) = self.manifest.take() {
            let manifest = runtime.store_session_manifest(manifest);
            self.publish(runtime, manifest, None);
            for value in std::mem::take(&mut self.deferred) {
                self.dispatch_value(value, runtime);
            }
        }
        let mut bytes = bytes;
        if self.discarding_oversized_line {
            let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') else {
                return Ok(());
            };
            self.discarding_oversized_line = false;
            bytes = &bytes[newline + 1..];
        }
        self.buffer.extend_from_slice(bytes);
        loop {
            let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') else {
                if self.buffer.len() > MAX_LINE_BYTES {
                    self.buffer.clear();
                    self.discarding_oversized_line = true;
                    let _ = runtime.publish_session_notice(
                        format!("Codex input line exceeded {MAX_LINE_BYTES} bytes."),
                        NoticeSeverity::Warning,
                    );
                }
                break;
            };
            let line: Vec<u8> = self.buffer.drain(..=newline).collect();
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let value = serde_json::from_slice::<Value>(line)
                .map_err(|error| format!("Malformed Codex output: {error}"))?;
            self.dispatch_value(value, runtime);
        }
        Ok(())
    }

    fn finish(&mut self, runtime: &Arc<SessionRuntime>) {
        self.permission_broker.close();
        // (S4-04) The transport is over, so no answer can arrive for a request
        // still waiting: failing them here is what stops a steer from waiting out
        // the whole `STEER_RESPONSE_TIMEOUT` after the provider is gone. The same
        // shape as Pi's `PiControl::fail_pending`.
        self.requests
            .fail_pending("Codex control channel closed before the response arrived.");
        if let Ok(mut ids) = self.response_ids.lock() {
            ids.clear();
        }
        if !self.buffer.is_empty() {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: "Codex ended with an unterminated output line.".to_string(),
                },
                None,
            );
        }
    }
}

fn send_permission_decision(
    stdin: &Mutex<Option<ChildStdin>>,
    id: Option<&Value>,
    decision: &str,
) -> io::Result<()> {
    let Some(id) = id else {
        return Ok(());
    };
    send_frame(stdin, &permission_decision_frame(id, decision), "Codex").map_err(wire_to_io)
}

fn send_result(stdin: &Mutex<Option<ChildStdin>>, id: &Value, result: Value) -> io::Result<()> {
    let frame = response_frame(id, result);
    let mut bytes = serde_json::to_vec(&frame)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    write_child_stdin(stdin, &bytes, "Codex")
}

fn response_frame(id: &Value, result: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn permission_decision_frame(id: &Value, decision: &str) -> Value {
    response_frame(id, serde_json::json!({ "decision": decision }))
}

struct CodexStderr {
    stderr: Option<ChildStderr>,
}

impl CodexStderr {
    fn start(stderr: ChildStderr) -> Self {
        Self {
            stderr: Some(stderr),
        }
    }
}

impl StderrSource for CodexStderr {
    fn spawn(mut self: Box<Self>, runtime: Arc<SessionRuntime>) -> io::Result<JoinHandle<()>> {
        let mut stderr = self
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("Codex stderr drain was already consumed"))?;
        std::thread::Builder::new()
            .name("session-codex-stderr".to_string())
            .spawn(move || {
                let mut buffer = [0u8; 4096];
                loop {
                    match stderr.read(&mut buffer) {
                        Ok(0) => return,
                        Ok(length) => {
                            let data = String::from_utf8_lossy(&buffer[..length]).into_owned();
                            publish_stderr_line(&runtime, data);
                        }
                        Err(_) => return,
                    }
                }
            })
    }
}

/// One stderr chunk to the transcript (broker-4): the S7 belt reads the RAW line
/// (the marker carries no secret), then the line is published through the one
/// redactor ACP and Claude already use — a child that echoes its environment
/// must not land a bearer in any observer's transcript. Same shape as ACP's
/// `publish_stderr_line`, which keeps its own copy (working code, untouched).
fn publish_stderr_line(runtime: &SessionRuntime, data: String) {
    // S7 belt (never authoritative — the `mcpServerStatus` poll is): codex
    // admitting it dropped the config means whatever carrier was installed is
    // not in force. Unverified either way it is read — the marker can only
    // move caution-ward, never benign-ward.
    if data.contains("Invalid configuration; using defaults") {
        runtime.set_tools_state(crate::mcp_broker::ToolsState::Unverified);
    }
    let _ = runtime.publish_agent_event(
        SessionEvent::AgentStderr {
            data: runtime.redact_mcp_text(&data),
        },
        None,
    );
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::event_pull::ConnHandle;
    use super::super::permission_broker::PermissionBroker;
    use super::super::session_runtime::SessionRuntime;
    use super::{
        assert_codex_home, carried_image_paths, codex_delivery, codex_local_image_entry,
        decline_input_result, initialize_params, interrupt_params, mcp_launch, mode_values,
        notification_frame, parse_back_codex_config, permission_decision,
        permission_decision_frame, plan_codex_prompt, render_codex_config, request_frame,
        send_interrupt_request, steer_params_if_current, thread_start_params,
        turn_id_from_response, turn_start_params, turn_start_params_for_prompt,
        turn_start_params_with_images, turn_steer_params, validate_mode, write_codex_home,
        write_codex_home_with, CodexReader, CodexRequests, CodexSteerer, CODEX_HOME_ENV,
    };
    use crate::attachment_store::AttachmentStore;
    use crate::codex_view::{
        catalog_from_response, fixture_frames, CodexState, CodexStdout, CodexView,
    };
    use crate::raster_metadata::{clean_png, png_with_text_chunk, vector_input, vector_output};
    use crate::session::ReaderDispatch;
    use devboule_protocol::PromptAttachment;
    use devboule_protocol::SessionEvent;
    use devboule_protocol::WireError;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn method_frame(source: &str, method: &str) -> serde_json::Value {
        fixture_frames(source)
            .into_iter()
            .find(|frame| frame.get("method").and_then(serde_json::Value::as_str) == Some(method))
            .expect("measured request")
    }
    #[test]
    fn codex_modes_use_the_measured_policy_shapes() {
        assert_eq!(
            mode_values("auto"),
            serde_json::json!({
                "approvalPolicy": "on-request",
                "sandboxPolicy": {
                    "type": "workspaceWrite",
                    "networkAccess": false,
                    "writableRoots": []
                }
            })
            .as_object()
            .expect("object")
            .clone()
        );
        assert_eq!(
            mode_values("auto-review").get("approvalsReviewer"),
            Some(&serde_json::json!("auto_review"))
        );
        assert_eq!(
            mode_values("full-access"),
            serde_json::json!({
                "approvalPolicy": "never",
                "sandboxPolicy": { "type": "dangerFullAccess" }
            })
            .as_object()
            .expect("object")
            .clone()
        );
        assert!(validate_mode("bypass").is_err());
    }

    #[test]
    fn measured_control_frames_and_turn_modes_keep_the_wire_shapes() {
        let thread = method_frame(
            include_str!("../fixtures/wire/codex/E1-step3-thread.jsonl"),
            "thread/start",
        );
        let cwd = thread["params"]["cwd"].as_str().expect("cwd");
        assert_eq!(
            thread["params"],
            thread_start_params(Path::new(cwd), "auto")
        );

        let initialized = method_frame(
            include_str!("../fixtures/wire/codex/E1-step1-handshake.jsonl"),
            "initialized",
        );
        assert_eq!(
            initialized,
            notification_frame("initialized", serde_json::json!({}))
        );

        let changed = method_frame(
            include_str!("../fixtures/wire/codex/E1-step6-modechange.jsonl"),
            "turn/start",
        );
        let params = &changed["params"];
        assert_eq!(
            *params,
            turn_start_params(
                params["threadId"].as_str().expect("thread id"),
                params["input"][0]["text"].as_str().expect("prompt"),
                Some("full-access"),
                None,
                None,
            )
        );
        let interrupt = method_frame(
            include_str!("../fixtures/wire/codex/E1-step6b-interrupt.jsonl"),
            "turn/interrupt",
        );
        assert_eq!(
            interrupt["params"],
            interrupt_params(
                interrupt["params"]["threadId"].as_str().expect("thread id"),
                interrupt["params"]["turnId"].as_str().expect("turn id"),
            )
        );

        for (outcome, decision) in [
            (
                serde_json::json!({"outcome":{"outcome":"selected","optionId":"allow"}}),
                "accept",
            ),
            (
                serde_json::json!({"outcome":{"outcome":"selected","optionId":"deny"}}),
                "decline",
            ),
            (
                serde_json::json!({"outcome":{"outcome":"cancelled"}}),
                "cancel",
            ),
        ] {
            assert_eq!(permission_decision(&outcome), decision);
            let mut bytes =
                serde_json::to_vec(&permission_decision_frame(&serde_json::json!(5), decision))
                    .expect("response json");
            bytes.push(b'\n');
            let response: serde_json::Value = serde_json::from_slice(&bytes).expect("response");
            assert_eq!(response["result"]["decision"], decision);
        }
    }

    #[test]
    fn turn_start_carries_the_policy_only_after_a_mode_change() {
        let catalog = catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog");
        let state = CodexState::new("thread".to_string(), catalog, "auto");
        assert_eq!(state.mode_override(), None);

        let first = turn_start_params(
            &state.thread_id(),
            "first",
            state.mode_override().as_deref(),
            None,
            None,
        );
        assert!(first.get("approvalPolicy").is_none());
        assert!(first.get("sandboxPolicy").is_none());

        state.set_mode("read-only").expect("set mode");
        let changed = turn_start_params(
            &state.thread_id(),
            "changed",
            state.mode_override().as_deref(),
            None,
            None,
        );
        assert_eq!(changed["approvalPolicy"], "on-request");
        assert_eq!(changed["sandboxPolicy"]["type"], "readOnly");

        // Paseo keeps `hasWorkflowModeOverride` set, so every later turn
        // re-sends the policy too.
        let later = turn_start_params(
            &state.thread_id(),
            "later",
            state.mode_override().as_deref(),
            None,
            None,
        );
        assert_eq!(later["sandboxPolicy"]["type"], "readOnly");
    }

    #[test]
    fn read_only_thread_start_sends_the_read_only_sandbox() {
        let params = thread_start_params(Path::new("C:\\work"), "read-only");
        assert_eq!(params["approvalPolicy"], "on-request");
        assert_eq!(params["sandbox"], "read-only");
    }

    #[test]
    fn initialize_request_includes_paseo_capabilities_on_the_wire() {
        let frame = super::request_frame("d-1", "initialize", initialize_params());
        let mut bytes = serde_json::to_vec(&frame).expect("initialize request");
        bytes.push(b'\n');
        let wire = std::str::from_utf8(&bytes).expect("initialize bytes");
        assert!(wire.contains(r#""experimentalApi":true"#));
        assert!(wire.contains(r#""mcpServerOpenaiFormElicitation":true"#));
        assert_eq!(
            frame["params"]["clientInfo"],
            serde_json::json!({
                "name": "codex_app_server_daemon",
                "title": "Codex App Server Daemon",
                "version": "0.0.0"
            })
        );
    }

    #[test]
    fn unknown_server_request_gets_a_method_not_supported_error() {
        use std::io::{BufRead, BufReader};

        // S4-06: the fake app-server is a `node` script, so this skips where
        // there is no node rather than failing there, like every other
        // node-backed test in this file.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        let mut child = std::process::Command::new("node")
            .args([
                "-e",
                "process.stdin.on('data', data => process.stdout.write(data))",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("node is required for the Codex request test");
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let catalog = catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog");
        let mut reader = CodexReader {
            buffer: Vec::new(),
            discarding_oversized_line: false,
            deferred: Vec::new(),
            manifest: None,
            state: Arc::new(CodexState::new("thread".to_string(), catalog, "auto")),
            view: CodexView::new(None),
            permission_broker: Arc::clone(&broker),
            response_ids: Arc::new(Mutex::new(HashMap::new())),
            stdin,
            next_id: Arc::new(AtomicU64::new(1)),
            requests: Arc::new(CodexRequests::new()),
        };
        let runtime = Arc::new(SessionRuntime::new());
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "server-17",
            "method": "codex/futureRequest",
            "params": {}
        });
        reader.dispatch_value(request, &runtime);
        let mut line = String::new();
        stdout.read_line(&mut line).expect("response");
        let response: serde_json::Value = serde_json::from_str(&line).expect("response json");
        assert_eq!(
            response,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": "server-17",
                "error": { "code": -32601, "message": "method not supported" }
            })
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn interrupt_without_a_current_turn_is_an_ok_noop() {
        let catalog = catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog");
        let state = CodexState::new("thread".to_string(), catalog, "auto");
        let stdin = Mutex::new(None);
        let next_id = AtomicU64::new(1);
        assert!(matches!(
            send_interrupt_request(&stdin, &next_id, &state),
            Ok(false)
        ));
        assert_eq!(next_id.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn turn_start_response_records_the_turn_before_started_notification() {
        let catalog = catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog");
        let state = CodexState::new("thread".to_string(), catalog, "auto");
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "d-7",
            "result": { "turn": { "id": "turn-7" } }
        });
        state.set_turn(turn_id_from_response(&response));
        assert_eq!(state.current_turn().as_deref(), Some("turn-7"));
    }

    #[test]
    fn declined_input_is_a_session_notice_and_keeps_the_decline_shapes() {
        assert_eq!(
            decline_input_result(&serde_json::json!({
                "method": "item/tool/requestUserInput"
            })),
            serde_json::json!({ "answers": {} })
        );
        assert_eq!(
            decline_input_result(&serde_json::json!({
                "method": "mcpServer/elicitation/request"
            })),
            serde_json::json!({ "action": "decline" })
        );

        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let runtime = Arc::new(SessionRuntime::new());
        runtime.stream.lock().unwrap().screen = None;
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach");
        conn.track_with_agent_replay(
            "s.codex.notice",
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        let mut reader = CodexReader {
            buffer: Vec::new(),
            discarding_oversized_line: false,
            deferred: Vec::new(),
            manifest: None,
            state: Arc::new(CodexState::new(
                "thread".to_string(),
                catalog_from_response(&serde_json::json!({
                    "data": [{ "id": "model", "isDefault": true }]
                }))
                .expect("catalog"),
                "auto",
            )),
            view: CodexView::new(None),
            permission_broker: Arc::clone(&broker),
            response_ids: Arc::new(Mutex::new(HashMap::new())),
            stdin: Arc::new(Mutex::new(None)),
            next_id: Arc::new(AtomicU64::new(1)),
            requests: Arc::new(CodexRequests::new()),
        };
        reader.dispatch_value(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": "server-1",
                "method": "item/tool/requestUserInput",
                "params": {}
            }),
            &runtime,
        );
        let events = conn.pull_events();
        assert!(events.iter().any(|event| matches!(
            event.envelope.event,
            SessionEvent::SessionNotice { ref text, severity }
                if text == "Codex requested user input; Devboule declined it."
                    && severity == devboule_protocol::NoticeSeverity::Info
        )));
        assert!(!events
            .iter()
            .any(|event| matches!(event.envelope.event, SessionEvent::AgentMessage { .. })));
    }

    // --- image delivery (the static route) --------------------------------
    //
    // The routing decision lives in `plan_codex_prompt`, tested here
    // against the attachment store directly, without spawning a child — the
    // same arrangement the ACP sibling seam's tests use. The wire shape of
    // one entry is pinned against the live-verified `localImage` input:
    // `{"type":"localImage","path":...}` with no `detail`.

    struct PlanTempDir(std::path::PathBuf);

    impl PlanTempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let dir = std::env::temp_dir().join(format!(
                "devboule-codex-plan-{}-{}-{}",
                std::process::id(),
                tag,
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
    }

    impl Drop for PlanTempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn plan_attachment(name: &str, mime_type: &str, bytes: &[u8]) -> PromptAttachment {
        use base64::Engine;
        PromptAttachment {
            name: name.to_string(),
            mime_type: mime_type.to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    #[test]
    fn codex_delivery_is_the_static_variant() {
        // No handshake to negotiate with and deliberately no probe: an
        // unknown method on this surface answers `-32600`, not `-32601`, so
        // a method-not-found fallback would never fire. The format accepts
        // images, so the delivery is the static one the route reads.
        assert_eq!(
            codex_delivery(),
            super::super::ImageDelivery::StaticImageBlock
        );
    }

    #[test]
    fn a_capable_codex_prompt_plans_local_image_paths_and_no_path_line() {
        // Each raster becomes one `localImage` path; the text is the bare
        // user text, with no path line.
        let temp = PlanTempDir::new("capable");
        let store = AttachmentStore::new(&temp.0);
        let session_id = "codex-plan-capable";
        // A container the walk accepts but changes: what sits at the planned
        // path must be the stripped bytes, never the wire bytes.
        let sent = png_with_text_chunk();
        let kept = clean_png(0x01);
        assert_ne!(
            sent, kept,
            "the fixture must actually carry something that leaves"
        );
        let plan = plan_codex_prompt(
            &store,
            session_id,
            "describe this",
            &[plan_attachment("photo.png", "image/png", &sent)],
        )
        .expect("materialized")
        .expect("a raster plans paths");
        assert_eq!(plan.fallback_text, "describe this", "no path line");
        assert_eq!(carried_image_paths(Some(&plan)).len(), 1);
        let planned = carried_image_paths(Some(&plan))[0];
        assert_eq!(
            std::fs::read(planned).expect("read"),
            kept,
            "the path names the stripped file"
        );
        let params = turn_start_params_with_images(
            "thread-1",
            &plan.fallback_text,
            &plan.image_paths,
            None,
            None,
            None,
        );
        assert_eq!(params["threadId"], "thread-1");
        let input = params["input"].as_array().expect("input array");
        assert_eq!(input.len(), 2);
        assert_eq!(
            input[0],
            serde_json::json!({"type": "text", "text": "describe this"})
        );
        // The exact entry shape, pinned literally: `type` plus `path`, no
        // `detail`, and `localImage` — not the unmeasured `image` the schema
        // also lists.
        assert_eq!(input[1]["type"], "localImage");
        assert_eq!(
            input[1]["path"].as_str().expect("path"),
            planned.to_string_lossy(),
            "the entry names the materialized file"
        );
        assert!(input[1].get("detail").is_none(), "no unmeasured detail");
        let entry = codex_local_image_entry(planned);
        assert_eq!(entry["type"], "localImage");
        assert!(entry.get("detail").is_none());
    }

    #[test]
    fn an_svg_only_codex_prompt_plans_no_paths_and_still_builds_the_legacy_text() {
        // SVG takes no inline shape on this surface. The plan still answers
        // with the text, and that text is exactly what the legacy write would
        // have produced, which is why a prompt with no carried path can take
        // the route without moving a byte on the wire. (This test used to
        // assert `plan.is_none()`: the route answers with the text now, so
        // that the send path never walks the attachments twice.)
        let temp = PlanTempDir::new("svg-only");
        let store = AttachmentStore::new(&temp.0);
        let session_id = "codex-plan-svg-only";
        let source = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        let attachment = plan_attachment("drawing.svg", "image/svg+xml", source);
        let plan = plan_codex_prompt(
            &store,
            session_id,
            "logo",
            std::slice::from_ref(&attachment),
        )
        .expect("materialized")
        .expect("an SVG plans no path, but the plan still carries the text");
        assert!(plan.image_paths.is_empty(), "an SVG plans no path");
        let stored = store
            .session(session_id)
            .expect("session")
            .materialize(&attachment)
            .expect("stored");
        assert_eq!(
            plan.fallback_text,
            format!("logo\n\n[Image available at: {}]", stored.to_string_lossy()),
            "the plan's text is the legacy path line, byte for byte"
        );
    }

    #[test]
    fn the_static_route_builds_the_turn_the_plan_decided() {
        // The route's frame is the measured `turn/start`: one text entry, then
        // one `localImage` per carried path, and nothing else moved — the
        // thread id and the model still come from the live state.
        let catalog = catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog");
        let state = CodexState::new("thread".to_string(), catalog, "auto");
        let temp = PlanTempDir::new("route");
        let store = AttachmentStore::new(&temp.0);
        let plan = plan_codex_prompt(
            &store,
            "codex-route",
            "describe this",
            &[plan_attachment("photo.png", "image/png", &clean_png(0x51))],
        )
        .expect("materialized")
        .expect("a raster plans a path");
        assert_eq!(plan.image_paths.len(), 1);
        let carried = turn_start_params_for_prompt(&state, &plan.fallback_text, &plan.image_paths);
        assert_eq!(carried["threadId"], "thread");
        let input = carried["input"].as_array().expect("input array");
        assert_eq!(input.len(), 2, "the text entry, then the path");
        assert_eq!(
            input[0],
            serde_json::json!({ "type": "text", "text": "describe this" })
        );
        assert_eq!(input[1]["type"], "localImage");
        // No carried path: the text-only turn, one entry.
        let bare = turn_start_params_for_prompt(&state, "describe this", &[]);
        assert_eq!(bare["input"].as_array().expect("input array").len(), 1);
    }

    #[test]
    fn a_params_builder_without_images_is_the_text_only_one() {
        // The static route sends every planned prompt through the images
        // builder, including one whose paths are all path lines. With no
        // carried path it has to be the text-only `turn/start` this surface
        // has always sent: same keys, same values, same order.
        for (policy_mode, model, effort) in [
            (None, None, None),
            (Some("workspace-write"), Some("gpt-5-codex"), Some("high")),
        ] {
            assert_eq!(
                turn_start_params_with_images(
                    "thread-1",
                    "describe this",
                    &[],
                    policy_mode,
                    model,
                    effort,
                ),
                turn_start_params("thread-1", "describe this", policy_mode, model, effort),
                "no carried path must not move a key"
            );
        }
    }

    #[test]
    fn codex_turn_steer_frame_is_byte_exact() {
        let frame = request_frame(
            "d-7",
            "turn/steer",
            turn_steer_params("thread-1", "turn-2", "hello"),
        );
        assert_eq!(
            serde_json::to_vec(&frame).expect("frame"),
            br#"{"jsonrpc":"2.0","id":"d-7","method":"turn/steer","params":{"threadId":"thread-1","expectedTurnId":"turn-2","input":[{"type":"text","text":"hello"}]}}"#
        );
    }

    /// A `CodexState` on `thread-1` whose running turn is `turn_id`.
    fn state_on_turn(turn_id: &str) -> Arc<CodexState> {
        let catalog = catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog");
        let state = CodexState::new("thread-1".to_string(), catalog, "auto");
        state.set_turn(Some(turn_id.to_string()));
        Arc::new(state)
    }

    #[test]
    fn a_codex_steer_carries_the_turn_it_was_checked_for() {
        // The frame names the turn the caller captured as its precondition, so
        // Codex itself refuses a steer aimed at a turn that is over.
        let state = state_on_turn("turn-3");
        let params = steer_params_if_current(&state, "turn-3", "turn left")
            .expect("the captured turn is the current one");
        assert_eq!(params["threadId"], "thread-1");
        assert_eq!(params["expectedTurnId"], "turn-3");
        assert_eq!(params["input"][0]["text"], "turn left");
    }

    #[test]
    fn a_codex_steer_for_a_turn_that_has_moved_on_is_never_written() {
        // The write-time check, not the caller's earlier one: the state says
        // Codex is on `turn-4` while the steer was admitted for `turn-3`, and
        // nothing is written at all. The stdin here is absent, so a write
        // attempt would answer `Err` instead of `Ok(false)` — what this pins is
        // that the frame is never built, and no answer is ever waited for.
        let state = state_on_turn("turn-4");
        assert!(steer_params_if_current(&state, "turn-3", "turn left").is_none());
        let stdin: Arc<Mutex<Option<std::process::ChildStdin>>> = Arc::new(Mutex::new(None));
        let steerer = CodexSteerer {
            stdin,
            next_id: Arc::new(AtomicU64::new(1)),
            state,
            requests: Arc::new(CodexRequests::new()),
        };
        assert!(matches!(
            steerer.begin_steer("turn-3", "turn left"),
            Ok(None)
        ));
    }

    /// S4-04: the app-server's output ending fails every waiter still registered,
    /// so a steer answers `Err` — the fate is unknown — instead of waiting out
    /// the whole fifteen-second timeout for an answer that cannot come.
    #[test]
    fn a_codex_steer_is_not_left_waiting_when_the_app_server_ends() {
        use std::io::BufReader;
        use std::process::Stdio;

        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        // A fake Codex that reads one request and exits without answering it.
        let mut child = std::process::Command::new("node")
            .args(["-e", "process.stdin.once('data', () => process.exit(0))"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("node is required for the Codex end-of-transport test");
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let stdout = child.stdout.take().expect("stdout");
        let requests = Arc::new(CodexRequests::new());
        let mut steerer = CodexSteerer {
            stdin,
            next_id: Arc::new(AtomicU64::new(7)),
            state: state_on_turn("turn-3"),
            requests: Arc::clone(&requests),
        };
        let mut reader = CodexReader {
            buffer: Vec::new(),
            discarding_oversized_line: false,
            deferred: Vec::new(),
            manifest: None,
            state: state_on_turn("turn-3"),
            view: CodexView::new(None),
            permission_broker: PermissionBroker::for_test(Arc::new(|_, _| Ok(()))),
            response_ids: Arc::new(Mutex::new(HashMap::new())),
            stdin: Arc::new(Mutex::new(None)),
            next_id: Arc::new(AtomicU64::new(1)),
            requests: Arc::clone(&requests),
        };
        // The reader runs the real end-of-transport path: read to EOF, then
        // `finish`, which is where the waiters are failed.
        let runtime = Arc::new(SessionRuntime::new());
        let reader_runtime = Arc::clone(&runtime);
        let reader_thread = std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            let mut bytes = Vec::new();
            let _ = std::io::Read::read_to_end(&mut stdout, &mut bytes);
            reader.finish(&reader_runtime);
        });

        let started = Instant::now();
        let answer = crate::test_support::steer_through_the_turn(&mut steerer, "turn left");
        let error = match answer {
            Some(Err(error)) => error,
            other => panic!("the transport is over: expected an error, got {other:?}"),
        };
        assert!(
            error.message.contains("control channel closed"),
            "the failure is the transport ending, not a timeout: {}",
            error.message
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the waiter was failed by the EOF, not by its own fifteen-second timeout"
        );
        let _ = child.wait();
        reader_thread.join().expect("the reader thread");
    }

    /// Spawn a fake Codex that echoes each line it reads back, so a test can
    /// read the frame the client wrote. The caller must have checked `node`
    /// first (`external_program_skip_reason`).
    fn spawn_codex_echoing() -> std::process::Child {
        std::process::Command::new("node")
            .args([
                "-e",
                "process.stdin.on('data', data => process.stdout.write(data))",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("node is required for the Codex steer tests")
    }

    /// Spawn a fake Codex that answers each `turn/steer` with `body`, whose `id`
    /// it fills with the request's own — the app-server's shape, so the test
    /// exercises the real id-correlated delivery. The caller must have checked
    /// `node` first (`external_program_skip_reason`).
    fn spawn_codex_answering(body: &str) -> std::process::Child {
        let script = r#"
let buf = '';
process.stdin.on('data', data => {
  buf += data;
  let i;
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i);
    buf = buf.slice(i + 1);
    let request;
    try { request = JSON.parse(line); } catch (error) { continue; }
    if (request.method === 'turn/steer') {
      const answer = __ANSWER__;
      answer.id = request.id;
      process.stdout.write(JSON.stringify(answer) + '\n');
    }
  }
});
"#
        .replace("__ANSWER__", body);
        std::process::Command::new("node")
            .args(["-e", &script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("node is required for the Codex steer tests")
    }

    /// One steer against a fake Codex that answers `body`: the child, the real
    /// delivery (`CodexRequests::deliver`, which is what the reader calls) and
    /// the steer all run, so the answer the steerer returns is the one the
    /// response produced.
    fn codex_steer_against(body: &str) -> Result<bool, WireError> {
        let requests = Arc::new(CodexRequests::new());
        let mut child = spawn_codex_answering(body);
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let stdout = child.stdout.take().expect("stdout");
        let delivered = Arc::clone(&requests);
        let reader = std::thread::spawn(move || {
            use std::io::BufRead;
            let mut stdout = std::io::BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match stdout.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let value: serde_json::Value =
                    serde_json::from_str(line.trim()).expect("the answer the fake Codex wrote");
                assert!(
                    delivered.deliver(&value),
                    "the answer named an id no waiter registered"
                );
            }
        });
        let mut steerer = CodexSteerer {
            stdin,
            next_id: Arc::new(AtomicU64::new(7)),
            state: state_on_turn("turn-3"),
            requests,
        };
        let answer = crate::test_support::steer_through_the_turn(&mut steerer, "turn left");
        let _ = child.kill();
        let _ = child.wait();
        reader.join().expect("the fake Codex reader");
        answer.expect("the turn was running at admission")
    }

    #[test]
    fn a_codex_steer_is_accepted_only_by_a_response_for_the_steered_turn() {
        // A2-03: the write is a request, not a decision. The acceptance is the
        // app-server's own response, and only one that names the turn the steer
        // was written into counts as one.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        assert!(
            matches!(
                codex_steer_against(r#"{"jsonrpc":"2.0","result":{"turn":{"id":"turn-3"}}}"#),
                Ok(true)
            ),
            "the response naming the steered turn is the acceptance"
        );
    }

    #[test]
    fn a_codex_steer_response_for_another_turn_is_a_refusal_not_an_acceptance() {
        // The response says the app-server took a steer — for a *different*
        // turn. Answering `Ok(true)` here would tell the caller its text landed
        // in the turn it was admitted for when the provider said otherwise.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        assert!(matches!(
            codex_steer_against(r#"{"jsonrpc":"2.0","result":{"turn":{"id":"turn-9"}}}"#),
            Ok(false)
        ));
    }

    #[test]
    fn a_codex_steer_answered_by_an_error_or_no_turn_is_a_refusal() {
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        assert!(
            matches!(
                codex_steer_against(
                    r#"{"jsonrpc":"2.0","error":{"code":-32600,"message":"steer refused"}}"#
                ),
                Ok(false)
            ),
            "an error result is not a steer"
        );
        assert!(
            matches!(
                codex_steer_against(r#"{"jsonrpc":"2.0","result":{}}"#),
                Ok(false)
            ),
            "a response that names no turn is not a steer for this one"
        );
    }

    #[test]
    fn a_codex_steer_that_is_still_current_is_written_with_its_precondition() {
        // The frame itself, read back from the child: the request names the turn
        // the caller captured as its precondition, so Codex refuses a steer
        // aimed at a turn that is over. The answer is not read here — this pins
        // the bytes, and `CodexRequests` is what turns them into a decision.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        use std::io::{BufRead, BufReader};

        let mut child = spawn_codex_echoing();
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let steerer = CodexSteerer {
            stdin,
            next_id: Arc::new(AtomicU64::new(7)),
            state: state_on_turn("turn-3"),
            requests: Arc::new(CodexRequests::new()),
        };
        let request = steerer
            .begin_steer("turn-3", "turn left")
            .expect("the frame was written");
        assert!(
            request.is_some(),
            "the current turn is the one the steer is written for"
        );
        let mut line = String::new();
        stdout
            .read_line(&mut line)
            .expect("the frame the child read");
        let frame: serde_json::Value = serde_json::from_str(&line).expect("frame json");
        assert_eq!(frame["jsonrpc"], "2.0");
        assert_eq!(frame["method"], "turn/steer");
        assert_eq!(frame["params"]["expectedTurnId"], "turn-3");
        assert_eq!(frame["params"]["input"][0]["text"], "turn left");
        assert_eq!(
            frame["id"], "d-7",
            "the request id is the one its answer will name"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn an_svg_keeps_its_path_line_beside_codex_image_paths() {
        // A mixed prompt carries both: the raster as a `localImage` path,
        // the SVG as a path line in the text.
        let temp = PlanTempDir::new("mixed");
        let store = AttachmentStore::new(&temp.0);
        let source = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        let plan = plan_codex_prompt(
            &store,
            "codex-plan-mixed",
            "logo and photo",
            &[
                plan_attachment("photo.png", "image/png", &clean_png(0x13)),
                plan_attachment("drawing.svg", "image/svg+xml", source),
            ],
        )
        .expect("materialized")
        .expect("the raster plans a path");
        assert_eq!(carried_image_paths(Some(&plan)).len(), 1);
        assert!(
            plan.fallback_text
                .starts_with("logo and photo\n\n[Image available at: "),
            "{}",
            plan.fallback_text
        );
        assert!(
            plan.fallback_text.ends_with(".svg]"),
            "{}",
            plan.fallback_text
        );
        assert!(
            !plan.fallback_text.contains(".png]"),
            "the raster left no path line: {}",
            plan.fallback_text
        );
        let params = turn_start_params_with_images(
            "thread-1",
            &plan.fallback_text,
            &plan.image_paths,
            None,
            None,
            None,
        );
        let input = params["input"].as_array().expect("array");
        assert_eq!(input.len(), 2);
        assert!(input[0]["text"].as_str().expect("text").ends_with(".svg]"));
        assert_eq!(input[1]["type"], "localImage");
    }

    #[test]
    fn a_jpeg_plans_its_stripped_path_on_codex() {
        const EXIF_JPEG_VECTOR: &str =
            "a jpeg whose APP1 holds EXIF, between a kept JFIF APP0 and a kept ICC APP2";
        let temp = PlanTempDir::new("jpeg");
        let store = AttachmentStore::new(&temp.0);
        let sent = vector_input(EXIF_JPEG_VECTOR);
        let kept = vector_output(EXIF_JPEG_VECTOR);
        assert_ne!(sent, kept, "the vector must actually strip something");
        let plan = plan_codex_prompt(
            &store,
            "codex-plan-jpeg",
            "describe this",
            &[plan_attachment("photo.jpg", "image/jpeg", &sent)],
        )
        .expect("materialized")
        .expect("a JPEG plans a path");
        assert_eq!(plan.fallback_text, "describe this");
        let planned = carried_image_paths(Some(&plan))[0];
        assert_eq!(
            std::fs::read(planned).expect("read"),
            kept,
            "stripped JPEG bytes at the planned path"
        );
    }
    // ---- S6: CODEX_HOME carrier ----

    #[test]
    fn codex_home_config_round_trips_and_refuses_garbage() {
        // Typed values render the probe-measured shape; the parse-back accepts
        // exactly our server entry and refuses everything else.
        let text = render_codex_config("http://127.0.0.1:4321/mcp");
        let server = parse_back_codex_config(&text).expect("our bytes parse back");
        assert_eq!(server.url, "http://127.0.0.1:4321/mcp");
        assert_eq!(
            server.bearer_token_env_var,
            crate::mcp_broker::MCP_TOKEN_ENV
        );
        for bad in [
            "this is not valid toml [",
            "",
            "model = \"x\"\n",
            "[mcp_servers.other]\nurl = \"http://127.0.0.1:1/mcp\"\nbearer_token_env_var = \"DEVBOULE_MCP_TOKEN\"\n",
            "[mcp_servers.devboule]\nurl = \"\"\nbearer_token_env_var = \"DEVBOULE_MCP_TOKEN\"\n",
            "[mcp_servers.devboule]\nurl = \"http://127.0.0.1:1/mcp\"\nbearer_token_env_var = \"SOMEONE_ELSES_TOKEN\"\n",
        ] {
            assert!(
                parse_back_codex_config(bad).is_err(),
                "parse-back refuses: {bad:?}"
            );
        }
    }

    #[test]
    fn codex_home_write_refuses_before_anything_lands() {
        // A writer emitting garbage is refused at preparation: no home dir, no
        // config file. Mutation: skip the parse-back in `write_codex_home_with`
        // → the garbage lands and this test is red.
        let dir = std::env::temp_dir().join(format!("devboule-codex-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("devboule-codex-home-0");
        assert!(write_codex_home_with(&home, "this is not valid toml [").is_err());
        assert!(!home.exists(), "a refused write leaves no home behind");
        // And the honest road lands a protected config with no secret on disk.
        write_codex_home(&home, "http://127.0.0.1:4321/mcp").expect("honest write");
        let config = home.join("config.toml");
        assert!(config.is_file());
        let text = std::fs::read_to_string(&config).expect("read back");
        assert!(text.contains("http://127.0.0.1:4321/mcp"));
        assert!(!text.contains("secret-bearer"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&config)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "carrier files are owner-only");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_home_assertion_is_canonicalised_both_sides() {
        // Raw string equality would refuse healthy children on Windows
        // (symlink/case normalisation); both sides canonicalise.
        let dir = std::env::temp_dir().join(format!("devboule-codex-echo-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let canonical = dir.canonicalize().expect("canonicalize");
        // A trailing separator spells the same dir: strings differ, homes do not.
        let with_sep = format!("{}{}", canonical.display(), std::path::MAIN_SEPARATOR);
        assert_codex_home(Some(&with_sep), &dir).expect("trailing separator is the same home");
        assert_codex_home(Some(&canonical.to_string_lossy()), &dir).expect("echo matches");
        let other = std::env::temp_dir();
        if other.canonicalize().expect("tmp") != canonical {
            assert!(
                assert_codex_home(Some(&other.to_string_lossy()), &dir).is_err(),
                "a different dir is refused, never run against"
            );
        }
        assert!(
            assert_codex_home(None, &dir).is_err(),
            "absent echo is refused"
        );
        assert!(
            assert_codex_home(Some(""), &dir).is_err(),
            "empty echo is refused"
        );
        assert!(
            assert_codex_home(
                Some(&canonical.to_string_lossy()),
                Path::new("devboule-no-such-dir-9f1a"),
            )
            .is_err(),
            "an unreadable expected home fails closed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_mcp_launch_separates_env_from_argv() {
        // S4 seam body for Codex: env carries the token value + the home path,
        // argv carries nothing, owned dirs name the sweepable home.
        let dir =
            std::env::temp_dir().join(format!("devboule-codex-launch-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let config = crate::mcp_broker::McpLaunchConfig::for_test(
            "http://127.0.0.1:4321/mcp",
            "secret-bearer-launch",
        );
        let carrier = mcp_launch(&config, &dir).expect("carrier");
        assert!(carrier.arg_additions.is_empty(), "no verbatim argv splice");
        let env: std::collections::HashMap<_, _> = carrier.env_additions.iter().cloned().collect();
        assert_eq!(
            env.get(crate::mcp_broker::MCP_TOKEN_ENV)
                .map(String::as_str),
            Some("secret-bearer-launch")
        );
        let home = env.get(CODEX_HOME_ENV).expect("CODEX_HOME rides the env");
        assert_eq!(carrier.owned_dirs.len(), 1);
        assert_eq!(
            carrier.owned_dirs[0].to_string_lossy(),
            home.as_str(),
            "the owned dir is the env dir"
        );
        assert!(
            home.contains("devboule-codex-home-"),
            "owned home name the sweep covers: {home}"
        );
        let text = std::fs::read_to_string(carrier.owned_paths[0].clone()).expect("config on disk");
        assert!(text.contains("http://127.0.0.1:4321/mcp"));
        assert!(!text.contains("secret-bearer-launch"), "no secret on disk");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fake `codex app-server` (node): answers initialize/model-list/thread-start,
    /// echoing the home from `FAKE_CODEX_HOME`, so the handshake assertion runs
    /// without the real binary. stderr is nulled: the real assertion reads the
    /// protocol echo, never the log.
    const FAKE_CODEX_HANDSHAKE: &str = r#"
const home = process.env.FAKE_CODEX_HOME || "";
let buf = "";
process.stdin.on("data", (chunk) => {
  buf += chunk.toString();
  let nl;
  while ((nl = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, nl);
    buf = buf.slice(nl + 1);
    if (!line.trim()) continue;
    let msg;
    try { msg = JSON.parse(line); } catch { continue; }
    if (msg.id === undefined || msg.id === null) continue;
    let result = {};
    if (msg.method === "initialize") result = { codexHome: home, userAgent: "fake-codex" };
    else if (msg.method === "model/list") result = { data: [{ id: "fake-model", isDefault: true }] };
    else if (msg.method === "thread/start") result = { thread: { id: "thread-fake" } };
    process.stdout.write(JSON.stringify({ id: msg.id, result }) + "\n");
  }
});
"#;

    fn fake_codex_child(home: &std::path::Path) -> std::process::Child {
        std::process::Command::new("node")
            .args(["-e", FAKE_CODEX_HANDSHAKE])
            .env("FAKE_CODEX_HOME", home)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("node is required for the fake Codex handshake")
    }

    #[test]
    fn codex_handshake_asserts_the_echoed_home_end_to_end() {
        // Full `perform_handshake` through a fake child: echo match proceeds,
        // echo mismatch refuses (never runs against the wrong home), and the
        // `None` road — today's production road — asserts nothing.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        let dir =
            std::env::temp_dir().join(format!("devboule-codex-handshake-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let other = std::env::temp_dir().join(format!(
            "devboule-codex-handshake-other-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&other);
        let run = |expected: Option<&std::path::Path>| {
            let mut child = fake_codex_child(&dir);
            let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
            let mut stdout =
                CodexStdout::spawn(child.stdout.take().expect("stdout")).expect("reader");
            let next_id = AtomicU64::new(1);
            let outcome =
                super::perform_handshake(&mut stdout, &stdin, &next_id, &dir, "auto", expected);
            let _ = child.kill();
            let _ = child.wait();
            outcome
        };
        let handshake = run(Some(&dir)).expect("echo match proceeds");
        assert_eq!(handshake.thread_id, "thread-fake");
        let error = match run(Some(&other)) {
            Err(error) => error,
            Ok(_) => panic!("echo mismatch refuses"),
        };
        assert!(
            error.message.contains("not the chosen home"),
            "the refusal names the mismatch: {}",
            error.message
        );
        run(None).expect("the None road asserts nothing");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn live_codex_recognises_the_home_carrier_and_reports_failure_honestly() {
        // S6 live (real codex-cli, unreachable broker): the handshake echo names
        // our home (carrier read), and `mcpServerStatus/list` shows the entry
        // with a `toolsError` — the probe's failure shape, never `connected`.
        // Skips where Codex is not runnable (gate PATH caveat, stated in report).
        let Some((program, args)) = live_codex_command() else {
            eprintln!("skipping: live codex is not runnable here");
            return;
        };
        let dir = std::env::temp_dir().join(format!("devboule-codex-live-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let home = dir.join("devboule-codex-home-live");
        super::write_codex_home(&home, "http://127.0.0.1:9/mcp").expect("live home written");
        let status = live_codex_result(
            &program,
            &args,
            &home,
            "live-canary-token",
            "mcpServerStatus/list",
            serde_json::json!({}),
        );
        let entries = status["data"].as_array().expect("status data array");
        assert_eq!(entries.len(), 1, "the carrier entry is listed: {status}");
        assert_eq!(entries[0]["name"], "devboule");
        assert!(
            entries[0]["toolsError"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "configured-and-failed reads from toolsError, never runtimeStatus: {status}"
        );
        assert!(
            entries[0]["runtimeStatus"].is_null(),
            "the probe's null-status trap holds live: {status}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_codex_sends_the_bearer_and_serves_a_catalog_without_tools_error() {
        // S6/S7 live (real codex-cli + local stub broker): the token the daemon
        // put in the child env flies on the stub's wire, argv stays clean, and
        // `mcpServerStatus/list` reports the entry with a tools catalog and a null
        // `toolsError` — the success shape the probe left unmeasured (Q2, closed
        // by the +12 s two-poll scratch: `runtimeStatus` stays null even fully
        // working, so the criterion keys on catalog + null error, never status).
        // Skips where Codex is not runnable.
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicBool, Ordering};
        let Some((program, args)) = live_codex_command() else {
            eprintln!("skipping: live codex is not runnable here");
            return;
        };
        let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("stub binds");
        listener.set_nonblocking(true).expect("stub nonblocking");
        let port = listener.local_addr().expect("stub port").port();
        let stub_url = format!("http://127.0.0.1:{port}/mcp");
        let stub_seen = Arc::clone(&seen);
        let stub_stop = Arc::clone(&stop);
        let stub = std::thread::spawn(move || {
            let mut served = 0;
            while !stub_stop.load(Ordering::Acquire) && served < 24 {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                let end = loop {
                    match stream.read(&mut byte) {
                        Ok(0) => break None,
                        Ok(_) => {
                            head.extend_from_slice(&byte);
                            if head.len() > 65536 {
                                break None;
                            }
                            if head.windows(4).any(|w| w == b"\r\n\r\n") {
                                break Some(head.len());
                            }
                        }
                        Err(_) => break None,
                    }
                };
                let Some(end) = end else { continue };
                let head_text = String::from_utf8_lossy(&head[..end]).into_owned();
                let length = head_text
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.trim()
                            .eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                let mut body = vec![0u8; length.min(65536)];
                let mut read = 0;
                while read < body.len() {
                    match stream.read(&mut body[read..]) {
                        Ok(0) => break,
                        Ok(n) => read += n,
                        Err(_) => break,
                    }
                }
                let auth = head_text
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.trim()
                            .eq_ignore_ascii_case("authorization")
                            .then(|| value.trim().to_string())
                    })
                    .unwrap_or_default();
                let message: serde_json::Value =
                    serde_json::from_slice(&body[..read]).unwrap_or(serde_json::Value::Null);
                let method = message
                    .get("method")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string();
                stub_seen
                    .lock()
                    .expect("stub log")
                    .push((auth, method.clone()));
                let id = message
                    .get("id")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let (status, body) = if method == "notifications/initialized" {
                    ("202 Accepted", String::new())
                } else if method == "initialize" {
                    (
                        "200 OK",
                        serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "protocolVersion": "2025-06-18", "capabilities": {},
                                "serverInfo": { "name": "stub", "version": "1" },
                            },
                        })
                        .to_string(),
                    )
                } else if method == "tools/list" {
                    (
                        "200 OK",
                        serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": { "tools": [
                                { "name": "stub_tool", "description": "A stub tool.",
                                  "inputSchema": { "type": "object", "properties": {} } },
                            ] },
                        })
                        .to_string(),
                    )
                } else {
                    (
                        "200 OK",
                        serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "content": [{ "type": "text", "text": "stub-result" }],
                                "isError": false,
                            },
                        })
                        .to_string(),
                    )
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                served += 1;
            }
        });
        let dir = std::env::temp_dir().join(format!("devboule-codex-stub-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let home = dir.join("devboule-codex-home-stub");
        super::write_codex_home(&home, &stub_url).expect("stub home written");
        let started = Instant::now();
        let status = live_codex_result(
            &program,
            &args,
            &home,
            "live-bearer-xyz",
            "mcpServerStatus/list",
            serde_json::json!({}),
        );
        let elapsed = started.elapsed();
        stop.store(true, Ordering::Release);
        let _ = stub.join();
        // The golden success shape (Q2, measured live +12 s apart with identical
        // results): present, catalog served, no toolsError. `runtimeStatus` is
        // null even fully working — keyed on nothing, documented here.
        let entries = status["data"].as_array().expect("status data array");
        assert_eq!(entries.len(), 1, "the stub entry is listed: {status}");
        assert_eq!(entries[0]["name"], "devboule");
        assert!(
            entries[0]["tools"].get("stub_tool").is_some(),
            "the stub catalog arrives: {status}"
        );
        assert!(
            entries[0]
                .get("toolsError")
                .is_none_or(|value| value.is_null()),
            "no toolsError on success: {status}"
        );
        // The token the daemon put in the child env flies on the stub's wire.
        let seen = seen.lock().expect("stub log");
        assert!(!seen.is_empty(), "the child dialed the stub");
        assert!(
            seen.iter()
                .any(|(auth, _)| auth == "Bearer live-bearer-xyz"),
            "Bearer equals the env token: {seen:?}"
        );
        // And it never rode argv: our launch line carries no secret, and the
        // config names the env var without holding the value.
        assert!(
            !args.iter().any(|arg| arg.contains("live-bearer-xyz")),
            "argv is token-free"
        );
        let config = std::fs::read_to_string(home.join("config.toml")).expect("config");
        assert!(config.contains("DEVBOULE_MCP_TOKEN"));
        assert!(!config.contains("live-bearer-xyz"));
        eprintln!("live stub handshake+status took {elapsed:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- S7: post-spawn verification ----

    fn golden_status() -> serde_json::Value {
        // The Q2 golden, byte-shaped like the live stub answered (status null
        // even working — keyed on nothing).
        serde_json::json!({
            "data": [{
                "name": "devboule",
                "runtimeStatus": null,
                "tools": { "stub_tool": { "name": "stub_tool" } },
                "toolsError": null,
            }],
        })
    }

    #[test]
    fn codex_status_mapping_keys_on_presence_catalog_and_error() {
        use crate::mcp_broker::ToolsState;
        // Golden: present + catalog + null error → the only Hosted.
        assert_eq!(
            super::map_codex_status(&golden_status()),
            ToolsState::Hosted
        );
        // Configured and failed → established without working tools.
        let mut failed = golden_status();
        failed["data"][0]["toolsError"] = serde_json::json!("MCP startup failed: refused");
        assert_eq!(super::map_codex_status(&failed), ToolsState::Unverified);
        // Absent from data (never configured, or vanished): not proof of absence.
        assert_eq!(
            super::map_codex_status(&serde_json::json!({ "data": [] })),
            ToolsState::Unverified
        );
        assert_eq!(
            super::map_codex_status(&serde_json::json!({})),
            ToolsState::Unverified
        );
        // No error but no catalog either: no evidence either way.
        let mut empty = golden_status();
        empty["data"][0]["tools"] = serde_json::json!({});
        assert_eq!(super::map_codex_status(&empty), ToolsState::Unverified);
        // A different server's health says nothing about ours.
        assert_eq!(
            super::map_codex_status(
                &serde_json::json!({ "data": [{ "name": "other", "runtimeStatus": "connected",
                    "tools": { "t": {} }, "toolsError": null }] })
            ),
            ToolsState::Unverified
        );
        // Empty-string error reads as null (no error), not as failure.
        let mut blank = golden_status();
        blank["data"][0]["toolsError"] = serde_json::json!("");
        assert_eq!(super::map_codex_status(&blank), ToolsState::Hosted);
    }

    /// A fake child that answers one `mcpServerStatus/list` with `answer`, then
    /// goes quiet: the verification waiter gets exactly one chance.
    fn verify_harness(
        answer: Option<serde_json::Value>,
    ) -> (
        super::CodexVerifyBundle,
        std::thread::JoinHandle<()>,
        std::process::Child,
        std::sync::Arc<super::CodexRequests>,
    ) {
        let script = if let Some(answer) = answer {
            "let b='';process.stdin.on('data',c=>{b+=c;let n;while((n=b.indexOf('\\n'))>=0){const l=b.slice(0,n);b=b.slice(n+1);if(!l.trim())continue;let m;try{m=JSON.parse(l)}catch{continue}if(m.id===undefined||m.id===null)continue;process.stdout.write(JSON.stringify({id:m.id,result:ANSWER})+'\\n');}});"
                .replace("ANSWER", &serde_json::to_string(&answer).expect("answer"))
        } else {
            "process.stdin.on('data',()=>{});setTimeout(()=>{},30000); void 0;".to_string()
        };
        let mut child = std::process::Command::new("node")
            .args(["-e", &script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("node is required for the Codex verify harness");
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let stdout = child.stdout.take().expect("stdout");
        let requests = Arc::new(super::CodexRequests::new());
        let bundle = super::CodexVerifyBundle {
            stdin: Arc::clone(&stdin),
            next_id: Arc::new(AtomicU64::new(1)),
            requests: Arc::clone(&requests),
        };
        // The reader's role: lines off stdout into id-matched delivery.
        let pump_requests = Arc::clone(&requests);
        let pump = std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                pump_requests.deliver(&value);
            }
        });
        (bundle, pump, child, requests)
    }

    #[test]
    fn codex_verify_runs_off_thread_and_maps_the_golden_to_hosted() {
        // S7 threading (Q6): the poll runs on a worker while delivery is pumped
        // elsewhere — the reentrancy rule as executable proof. The signature
        // (bundle, never `&mut CodexReader`) is the compile-level half.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        let (bundle, pump, mut child, _) = verify_harness(Some(golden_status()));
        let worker = std::thread::spawn(move || {
            super::verify_codex_mcp_with_timeout(&bundle, Duration::from_secs(10))
        });
        let state = worker.join().expect("verify thread joins");
        assert_eq!(
            state,
            crate::mcp_broker::ToolsState::Hosted,
            "the golden poll establishes Hosted"
        );
        let _ = child.kill();
        let _ = child.wait();
        pump.join().expect("pump drains");
    }

    #[test]
    fn codex_verify_degrades_on_timeout_transport_end_and_dead_stdin() {
        use crate::mcp_broker::ToolsState;
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        // A black hole: bounded by the timeout, never a hang, and the child
        // survives the observation (verification is not a fate).
        let (bundle, pump, mut child, _) = verify_harness(None);
        let started = Instant::now();
        assert_eq!(
            super::verify_codex_mcp_with_timeout(&bundle, Duration::from_secs(2)),
            ToolsState::Unverified
        );
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "the wait is bounded, never a hang"
        );
        assert!(
            child.try_wait().expect("poll child").is_none(),
            "a timed-out verification leaves the child alive"
        );
        let _ = child.kill();
        let _ = child.wait();
        pump.join().expect("pump drains");
        // A transport end wakes the waiter fast with Unverified, never a hang:
        // parked first (polled for determinism), then failed.
        let (bundle, pump, mut child, requests) = verify_harness(None);
        let worker = std::thread::spawn(move || {
            super::verify_codex_mcp_with_timeout(&bundle, Duration::from_secs(30))
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while requests.pending_count() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            requests.pending_count(),
            1,
            "the poll parked exactly one waiter"
        );
        requests.fail_pending("Codex control channel closed before the response arrived.");
        let started = Instant::now();
        assert_eq!(
            worker.join().expect("woken waiter joins"),
            ToolsState::Unverified
        );
        assert!(
            started.elapsed() < Duration::from_secs(25),
            "the transport end wakes the waiter; it never sits out the timeout"
        );
        let _ = child.kill();
        let _ = child.wait();
        pump.join().expect("pump drains");
        // A dead stdin refuses at the write: Unverified, immediately.
        let dead = super::CodexVerifyBundle {
            stdin: Arc::new(Mutex::new(None)),
            next_id: Arc::new(AtomicU64::new(1)),
            requests: Arc::new(super::CodexRequests::new()),
        };
        assert_eq!(
            super::verify_codex_mcp_with_timeout(&dead, Duration::from_secs(5)),
            ToolsState::Unverified
        );
    }

    #[test]
    fn codex_verify_bundle_comes_with_the_carrier_only() {
        // Which spawns verify is a one-line rule, pinned without spawning.
        let stdin = Arc::new(Mutex::new(None));
        let next_id = Arc::new(AtomicU64::new(1));
        let requests = Arc::new(super::CodexRequests::new());
        assert!(
            super::codex_verify_bundle_for(&None, &stdin, &next_id, &requests).is_none(),
            "today's only road verifies nothing"
        );
        assert!(
            super::codex_verify_bundle_for(
                &Some(crate::mcp_broker::McpProviderConfig::default()),
                &stdin,
                &next_id,
                &requests
            )
            .is_some(),
            "a minted carrier is verified"
        );
    }

    #[test]
    fn codex_stderr_belt_marks_unverified_on_invalid_configuration() {
        // S7 belt (never authoritative): the exact stderr line the probe measured
        // flips the runtime Unverified; anything else leaves it alone. A real
        // child writes it, so this runs the drain, not the predicate.
        use crate::session::StderrSource;
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        let run = |line: &str| {
            let script = format!(
                "console.error({});",
                serde_json::to_string(line).expect("quote")
            );
            let mut child = std::process::Command::new("node")
                .args(["-e", &script])
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("node writes stderr");
            let stderr = child.stderr.take().expect("stderr");
            let runtime = Arc::new(super::super::session_runtime::SessionRuntime::new());
            let handle = Box::new(super::CodexStderr::start(stderr)).spawn(Arc::clone(&runtime));
            let _ = child.wait();
            handle.expect("drain joins").join().expect("drain returns");
            runtime.tools_state()
        };
        assert_eq!(
            run("ERROR codex_app_server: Invalid configuration; using defaults."),
            crate::mcp_broker::ToolsState::Unverified,
            "the probe's line trips the belt"
        );
        assert_eq!(
            run("some unrelated warning"),
            crate::mcp_broker::ToolsState::Unavailable,
            "anything else leaves the state alone"
        );
    }

    #[test]
    fn codex_verify_trigger_flips_the_runtime_detached() {
        // S8 trigger, driving the real `spawn_codex_verify_thread`: `None` is a
        // no-op; a carrier bundle flips the runtime when the poll lands, on a
        // thread that is not this one (the create path never waits for it).
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        let runtime = Arc::new(SessionRuntime::new());
        super::super::spawn_codex_verify_thread(None, &runtime, "s.verify.none");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            runtime.tools_state(),
            crate::mcp_broker::ToolsState::Unavailable,
            "no bundle changes nothing"
        );
        // A fake child answering the golden list, pumped like the reader would.
        let mut child = std::process::Command::new("node")
            .args(["-e",
                "let b='';process.stdin.on('data',c=>{b+=c;let n;while((n=b.indexOf('\\n'))>=0){const l=b.slice(0,n);b=b.slice(n+1);if(!l.trim())continue;let m;try{m=JSON.parse(l)}catch{continue}if(m.id===undefined||m.id===null)continue;process.stdout.write(JSON.stringify({id:m.id,result:{data:[{name:'devboule',runtimeStatus:null,tools:{t:{}},toolsError:null}]}})+'\\n');}});"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("node is required for the Codex trigger test");
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let stdout = child.stdout.take().expect("stdout");
        let requests = Arc::new(super::CodexRequests::new());
        let pump_requests = Arc::clone(&requests);
        let pump = std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                pump_requests.deliver(&value);
            }
        });
        let bundle = super::codex_verify_bundle_for(
            &Some(crate::mcp_broker::McpProviderConfig::default()),
            &stdin,
            &Arc::new(AtomicU64::new(1)),
            &requests,
        )
        .expect("a carrier verifies");
        super::super::spawn_codex_verify_thread(Some(bundle), &runtime, "s.verify.some");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if runtime.tools_state() == crate::mcp_broker::ToolsState::Hosted {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the detached flip lands: {:?}",
                runtime.tools_state()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        pump.join().expect("pump drains");
    }

    #[test]
    fn codex_none_road_installs_no_carrier_and_verifies_nothing() {
        // End-of-pass OFF property, executable: production's `None` road spawns
        // with no home, no extra env, and no verification bundle — a Codex
        // session obtains no bearer, no config file and no tool. (The gate
        // itself is pinned unit-level by `registration_is_a_fact`; this pins
        // the spawn road that S9 will light.)
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        let state = crate::server::ServerState::new("codex-none-road".to_string());
        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        let home = std::env::temp_dir().join(format!("devboule-codex-none-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&home);
        let command = crate::session::PtyCommand::new(
            "node",
            vec!["-e".to_string(), FAKE_CODEX_HANDSHAKE.to_string()],
            std::env::temp_dir(),
            vec![(
                "FAKE_CODEX_HOME".to_string(),
                home.to_string_lossy().into_owned(),
            )],
        );
        let mut spawned = super::spawn_process(
            &state,
            command,
            None,
            crate::profile_delivery::ProfileDelivery::none(),
        )
        .expect("the None road spawns exactly as before");
        assert!(
            spawned.pending_codex_verify.is_none(),
            "no carrier means no verification bundle"
        );
        spawned.killer.kill();
        let orphans: Vec<_> = std::fs::read_dir(&runtime_dir)
            .expect("runtime dir")
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("devboule-codex-home-"))
            })
            .collect();
        assert!(orphans.is_empty(), "no home prepared: {orphans:?}");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A fake child for the live carrier road: handshake answers plus a golden
    /// `mcpServerStatus/list` after `LIST_DELAY_MS`. The echo is globbed from
    /// `FAKE_RUNTIME_DIR` — the carrier home is minted inside `spawn_process`,
    /// so no caller can know its name beforehand and the assertion stays honest.
    const ROAD_FAKE: &str = r#"
const fs = require("fs"), path = require("path");
const runtimeDir = process.env.FAKE_RUNTIME_DIR || "";
const listDelay = parseInt(process.env.LIST_DELAY_MS || "0", 10);
function codexHome() {
  try {
    const hit = fs.readdirSync(runtimeDir).find((n) => n.startsWith("devboule-codex-home-"));
    return hit ? path.join(runtimeDir, hit) : "";
  } catch { return ""; }
}
let buf = "";
process.stdin.on("data", (chunk) => {
  buf += chunk.toString();
  let nl;
  while ((nl = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, nl);
    buf = buf.slice(nl + 1);
    if (!line.trim()) continue;
    let msg;
    try { msg = JSON.parse(line); } catch { continue; }
    if (msg.id === undefined || msg.id === null) continue;
    const reply = (result) => process.stdout.write(JSON.stringify({ id: msg.id, result }) + "\n");
    if (msg.method === "initialize") reply({ codexHome: codexHome(), userAgent: "fake-road" });
    else if (msg.method === "model/list") reply({ data: [{ id: "fake-model", isDefault: true }] });
    else if (msg.method === "thread/start") reply({ thread: { id: "thread-road" } });
    else if (msg.method === "mcpServerStatus/list") {
      const golden = { data: [{ name: "devboule", runtimeStatus: null, tools: { t: {} }, toolsError: null }] };
      if (listDelay > 0) setTimeout(() => reply(golden), listDelay);
      else reply(golden);
    }
  }
});
"#;

    #[test]
    fn codex_live_carrier_road_registers_verifies_and_lists() {
        // S9 wiring, end to end through production code: broker register (the
        // flipped gate admits Codex) → carrier → spawn (echo asserted) → bind
        // (Unverified installed) → detached verify → roster lists the child as
        // Hosted. The part-2 report's two uncovered lines — the trigger CALL
        // and the else-bind LINE — are both load-bearing here: without the
        // bind the word never leaves Unavailable, without the trigger it never
        // leaves Unverified.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        use crate::mcp_broker::ToolsState;
        let state = crate::server::ServerState::new("codex-road".to_string());
        let owner = devboule_protocol::OwnerId::new("local", "road").expect("owner");
        let id = "s.road.1";
        let guard = state
            .mcp
            .register_with_provider(
                id,
                &owner,
                &devboule_protocol::SessionKind::Codex,
                Some("codex"),
                crate::mcp_broker::AgentLineage::root(),
            )
            .expect("S9 registers Codex")
            .expect("a bearer is minted");
        assert!(state.mcp.is_registered(id));
        let config = state.mcp.launch_config(id).expect("launch config");
        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        let command = crate::session::PtyCommand::new(
            "node",
            vec!["-e".to_string(), ROAD_FAKE.to_string()],
            std::env::temp_dir(),
            vec![
                (
                    "FAKE_RUNTIME_DIR".to_string(),
                    runtime_dir.to_string_lossy().into_owned(),
                ),
                ("LIST_DELAY_MS".to_string(), "1500".to_string()),
            ],
        );
        let spawned = super::spawn_process(
            &state,
            command,
            Some(config),
            crate::profile_delivery::ProfileDelivery::none(),
        )
        .expect("the carrier road spawns with its echo asserted");
        assert!(
            spawned.pending_codex_verify.is_some(),
            "a minted carrier verifies"
        );
        let metadata = devboule_protocol::Session {
            id: id.to_string(),
            workspace_id: None,
            cwd: None,
            kind: devboule_protocol::SessionKind::Codex,
            title: "Road".to_string(),
            state: devboule_protocol::SessionState::Live { generation: 1 },
            elapsed_ms: Some(0),
            provider: Some("codex".to_string()),
            peer_session_id: None,
            created_at_ms: 1,
            origin: devboule_protocol::SessionOrigin::local(),
            display_name: Some("road".to_string()),
            created_by: None,
            profile_id: None,
            context_id: None,
            unattended: devboule_protocol::UnattendedState::No,
            labels: Default::default(),
        };
        crate::session::start_spawned_session(
            &state,
            &state.sessions,
            metadata,
            owner.clone(),
            None,
            None,
            spawned,
            Some(guard),
        )
        .expect("the road starts");
        // Unverified first (the else-bind ran), Hosted after the delayed poll
        // (the trigger ran): order matters, and the delay makes it deterministic.
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut saw_unverified = false;
        let hosted = loop {
            let entries = state.sessions.live_agent_entries(&owner).expect("roster");
            if let Some(entry) = entries.iter().find(|entry| entry.session.id == id) {
                let word = entry.runtime.tools_state();
                if word == ToolsState::Unverified {
                    saw_unverified = true;
                }
                if word == ToolsState::Hosted {
                    break true;
                }
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(
            saw_unverified,
            "bind installed Unverified before verify landed"
        );
        assert!(hosted, "the detached poll flipped the roster to Hosted");
        // Teardown removes the home with the session (the killer owns it).
        let _ = state.sessions.close(id, &owner, &None);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let orphans: Vec<_> = std::fs::read_dir(&runtime_dir)
                .expect("runtime dir")
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with("devboule-codex-home-"))
                })
                .collect();
            if orphans.is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "teardown removes the home: {orphans:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn codex_killer_removes_the_home_even_for_an_exited_child() {
        // The S9 road-test regression, pinned directly: a child that already
        // exited when `kill` runs must still lose its home. The old grace-loop
        // early `return` leaked the whole tree exactly here.
        let dir =
            std::env::temp_dir().join(format!("devboule-codex-killer-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let home = dir.join("devboule-codex-home-killed");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::write(home.join("config.toml"), b"stale").expect("config");
        let mut child = std::process::Command::new("node")
            .args(["-e", "process.exit(0);"])
            .spawn()
            .expect("node exits at once");
        // Reaped before kill: the grace loop observes the exit on entry.
        assert!(child.wait().expect("reap").success());
        let catalog = crate::codex_view::catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog");
        let mut killer = super::CodexKiller {
            process: Arc::new(Mutex::new(child)),
            stdin: Arc::new(Mutex::new(None)),
            next_id: Arc::new(AtomicU64::new(1)),
            state: Arc::new(crate::codex_view::CodexState::new(
                "thread-kill".to_string(),
                catalog,
                "auto",
            )),
            permission_broker: PermissionBroker::for_test(Arc::new(|_, _| Ok(()))),
            cancelled: Arc::new(AtomicBool::new(false)),
            codex_home: Some(home.clone()),
        };
        use crate::session::SessionKiller;
        killer.kill();
        assert!(!home.exists(), "an exited child still loses its home");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Attached-runtime helper mirroring ACP's: a runtime with a broker plus a
    /// subscription whose published events the test can pull back out.
    fn attached_runtime(
        session_id: &str,
        broker: Arc<PermissionBroker>,
    ) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
        let runtime = SessionRuntime::for_acp(session_id.to_string(), None, Arc::clone(&broker));
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach");
        conn.track_with_agent_replay(
            session_id,
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        (runtime, conn)
    }

    #[test]
    fn codex_bearer_is_redacted_from_stderr_before_delivery() {
        // Broker-4: a bearer-shaped secret planted in a Codex stderr line must
        // not reach any observer's transcript. The belt still reads the raw
        // marker line in the same call.
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let (runtime, conn) = attached_runtime("stderr-redaction-codex", broker);
        runtime.set_mcp_bearer("opaque-bearer-codex".to_string());
        runtime.set_mcp_url("http://127.0.0.1:4567/mcp".to_string());
        super::publish_stderr_line(
            &runtime,
            "codex echoed Bearer opaque-bearer-codex at http://127.0.0.1:4567/mcp".to_string(),
        );
        let event = conn
            .pull_events()
            .into_iter()
            .find_map(|event| match event.envelope.event {
                SessionEvent::AgentStderr { data } => Some(data),
                _ => None,
            })
            .expect("stderr event");
        assert_eq!(event, "codex echoed Bearer [redacted] at [redacted]");
        super::publish_stderr_line(
            &runtime,
            "ERROR codex_app_server: Invalid configuration; using defaults.".to_string(),
        );
        assert_eq!(
            runtime.tools_state(),
            crate::mcp_broker::ToolsState::Unverified,
            "the belt still reads the raw marker"
        );
    }

    #[test]
    fn codex_spawn_failure_removes_the_prepared_home() {
        // The 9th early-error path: the home exists before the child does, so a
        // spawn failure must remove it (no child exists to kill).
        let state = crate::server::ServerState::new("codex-early-error".to_string());
        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        let command = crate::session::PtyCommand::new(
            "devboule-no-such-program-9f1a",
            Vec::new(),
            std::env::temp_dir(),
            Vec::new(),
        );
        let config = crate::mcp_broker::McpLaunchConfig::for_test(
            "http://127.0.0.1:9/mcp",
            "early-error-token",
        );
        let error = match super::spawn_process(
            &state,
            command,
            Some(config),
            crate::profile_delivery::ProfileDelivery::none(),
        ) {
            Err(error) => error,
            Ok(_) => panic!("an unstartable program fails the spawn"),
        };
        assert!(
            error.message.contains("Could not start Codex"),
            "the refusal names the spawn: {}",
            error.message
        );
        let orphans: Vec<_> = std::fs::read_dir(&runtime_dir)
            .expect("runtime dir")
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("devboule-codex-home-"))
            })
            .collect();
        assert!(orphans.is_empty(), "no prepared home survives: {orphans:?}");
    }

    /// The real launch line, resolved exactly like production
    /// (`find_available("codex")` → shim-unwrapped `app_server_command`). `None`
    /// when Codex is not runnable here: the live tests skip, like the node tests.
    ///
    /// Deliberately NOT gated on spawning bare `codex`: on Windows CreateProcess
    /// cannot run npm's extensionless shim (probe-measured PATH trap), while the
    /// catalog resolves the shim to `node <script>` — so the catalog IS the gate.
    fn live_codex_command() -> Option<(std::path::PathBuf, Vec<String>)> {
        let agent = crate::provider_catalog::find_available("codex")?;
        let mut argv = agent.app_server_command?;
        if argv.is_empty() {
            return None;
        }
        let program = std::path::PathBuf::from(argv.remove(0));
        Some((program, argv))
    }

    /// Drive one app-server request against a live child and read the `result`.
    /// No thread needed: `initialize` + `mcpServerStatus/list` are pre-thread.
    fn live_codex_result(
        program: &std::path::Path,
        args: &[String],
        home: &std::path::Path,
        token: &str,
        method: &str,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let mut child = std::process::Command::new(program)
            .args(args)
            .env(super::CODEX_HOME_ENV, home)
            .env(crate::mcp_broker::MCP_TOKEN_ENV, token)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .current_dir(home)
            .spawn()
            .expect("live codex spawns");
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let mut stdout = CodexStdout::spawn(child.stdout.take().expect("stdout")).expect("reader");
        let next_id = AtomicU64::new(1);
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut deferred = Vec::new();
        // The id counter is internal (`d-N`); no caller id needed.
        let mut send = |method: &str, params: serde_json::Value| {
            super::request_response(
                &mut stdout,
                &stdin,
                &next_id,
                method,
                params,
                deadline,
                &mut deferred,
            )
            .unwrap_or_else(|_| panic!("live {method} answers"))
        };
        let initialize = send("initialize", super::initialize_params());
        super::assert_codex_home(
            initialize.get("codexHome").and_then(|value| value.as_str()),
            home,
        )
        .expect("live echo names the chosen home");
        // Notifications get no response, so they ride `send_frame`, never
        // `request_response` (which would park until the deadline). Mirrors
        // `perform_handshake`'s own `initialized` send.
        super::send_frame(
            &stdin,
            &super::notification_frame("initialized", serde_json::json!({})),
            "Codex",
        )
        .expect("live initialized notifies");
        let result = send(method, params);
        let _ = child.kill();
        let _ = child.wait();
        result
    }
}

#[cfg(test)]
mod delivery_tests {
    use super::{
        mode_answers_own_prompts, seed_model_and_effort, turn_start_params_for_prompt,
        validate_delivery, CodexState, ProfileDelivery,
    };
    use crate::codex_view::catalog_from_response;
    use devboule_protocol::SessionEvent;
    use std::sync::Arc;

    fn catalog() -> crate::codex_view::CodexCatalog {
        let frame = serde_json::json!({
            "data": [{
                "id": "gpt-5.1",
                "displayName": "GPT 5.1",
                "isDefault": true,
                "supportedReasoningEfforts": [
                    {"reasoningEffort": "high"},
                    {"reasoningEffort": "low"}
                ],
                "defaultReasoningEffort": "high",
            }]
        });
        catalog_from_response(&frame).expect("catalog")
    }

    /// The seed is the delivery: the first `turn/start` reads its model and
    /// effort from the state the profile seeded, so a seed that silently
    /// no-ops would put a child on the app-server's default model while the
    /// card named another.
    #[test]
    fn a_seeded_codex_delivery_reaches_the_first_turn_params() {
        let state = Arc::new(CodexState::new("thread".to_string(), catalog(), "auto"));
        let mut delivery = ProfileDelivery::none();
        delivery.model_id = Some("gpt-5.1".to_string());
        delivery.thinking_option_id = Some("low".to_string());
        seed_model_and_effort(&state, &delivery).expect("seeded");

        let params = turn_start_params_for_prompt(&state, "report your result", &[]);
        assert_eq!(
            params["model"], "gpt-5.1",
            "the first turn runs the profile's model"
        );
        assert_eq!(
            params["effort"], "low",
            "the first turn runs the profile's effort"
        );
    }

    fn delivery(mode: &str, auto_accept: bool) -> ProfileDelivery {
        let mut delivery = ProfileDelivery::for_request(Some(mode.to_string()));
        delivery.auto_accept = auto_accept;
        delivery
    }

    /// The daemon's broker answers the provider-agnostic ids it owns;
    /// `full-access` is this client's own knob, whose approval policy is
    /// `never` — the provider never asks anybody. Both admit an
    /// `autoAccept` tick; everything else asks the human.
    ///
    /// The broker half is **walked, not hand-listed** (the R2a audit's F9):
    /// the test iterates the table itself, so a fourth id added to
    /// `auto_answered_modes` is asserted to answer here the moment it
    /// exists, and a codex-side predicate change is caught against whatever
    /// the table holds.
    #[test]
    fn codex_auto_answer_modes_are_the_broker_list_plus_full_access() {
        assert!(
            mode_answers_own_prompts("full-access"),
            "approvalPolicy never"
        );
        for mode_id in crate::provider_catalog::auto_answered_modes() {
            assert!(
                mode_answers_own_prompts(mode_id),
                "route A: {mode_id} answers its own prompts"
            );
        }
        assert!(!mode_answers_own_prompts("auto"), "on-request asks");
        assert!(!mode_answers_own_prompts("read-only"), "on-request asks");
        assert!(
            !mode_answers_own_prompts("auto-review"),
            "eligible is not all: on-request requests may still reach the human"
        );
    }

    /// The predicate reads the row, not a name it spells itself: walked over
    /// every mode the manifest presents, it agrees with the table's own
    /// `unattended` answer. A new `Yes` row added to `CODEX_MODES` is
    /// answered here the moment it exists — the old `mode_id ==
    /// "full-access"` shape would go red on exactly that row, refusing a
    /// pair with a rationale false about it.
    #[test]
    fn auto_answer_agrees_with_the_table_on_every_presented_row() {
        let state = Arc::new(CodexState::new("thread".to_string(), catalog(), "auto"));
        let SessionEvent::SessionManifest {
            modes: Some(modes), ..
        } = state.manifest()
        else {
            panic!("the Codex manifest carries modes");
        };
        assert!(
            modes
                .available_modes
                .iter()
                .any(|mode| mode.id == "full-access"),
            "the walked table still carries the Yes row this pins"
        );
        for mode in &modes.available_modes {
            assert_eq!(
                mode_answers_own_prompts(&mode.id),
                crate::codex_view::unattended_answer(Some(mode.id.as_str()))
                    == devboule_protocol::UnattendedState::Yes,
                "{}: the predicate answers what the row's marker says",
                mode.id
            );
        }
    }

    /// The contradiction is refused at creation: a tick over a mode that
    /// asks the human names two ways to run and delivers neither.
    #[test]
    fn a_codex_auto_accept_tick_over_an_asking_mode_is_refused() {
        validate_delivery(&delivery("full-access", true)).expect("never asks");
        validate_delivery(&delivery("auto", false)).expect("asking mode, no tick");
        let error = validate_delivery(&delivery("auto", true))
            .expect_err("a tick over an asking mode is the contradiction");
        assert!(
            error.message.contains("contradict"),
            "the refusal names both halves: {}",
            error.message
        );
        assert!(
            error.message.contains("mode 'auto'"),
            "the refusal names the delivered mode: {}",
            error.message
        );
    }
}
