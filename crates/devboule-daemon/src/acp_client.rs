//! ACP stdio transport for live agent sessions.
//!
//! This module owns only the process and protocol adapters. The parent
//! session module still owns the runtime, attachment queue, coalescer,
//! journal, liveness monitor, registry and teardown order.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use agent_client_protocol::schema::v1::{
    ClientCapabilities, FileSystemCapabilities, Implementation, InitializeRequest,
};
use agent_client_protocol::schema::ProtocolVersion;
use devboule_protocol::{ErrorCode, PermissionOption, SessionEvent, WireError};

use super::acp_host::{AcpHost, RpcError, RpcRespond};
use crate::acp_view::{
    add_vendor_surface, catalog_from_config_options, classify_line, current_mode_id_from_update,
    has_standard_modes, merge_handshake_manifest, unmodeled_content_kind, view_from_envelope_in,
    AcpLineKind, ConfigOptionSurface, HandshakeManifest, ModelSwitchShape, PromptCapabilities,
};
use crate::mcp_broker::McpLaunchConfig;
use crate::paths::RuntimePaths;
use crate::process_tree::{JobObject, ProcessHandle};
use crate::server::ServerState;

use super::permission_broker::{
    PermissionBroker, MAX_ACP_PERMISSION_FIELD_BYTES, MAX_ACP_PERMISSION_OPTIONS,
};
use super::PtyCommand;
use super::{
    write_child_stdin, ModelSwitcher, ReaderDispatch, SessionKiller, SessionRuntime,
    SpawnedSession, StderrSource, StdioWaitableChild,
};

const COMMAND_ENV: &str = "DEVBOULE_ACP_COMMAND";
/// Test/direct-command counterpart to [`COMMAND_ENV`]. A direct command has
/// no catalog row to identify it; tests set this to the stub provider id so a
/// later resume can still exercise the named-provider path.
const COMMAND_PROVIDER_ENV: &str = "DEVBOULE_ACP_PROVIDER_ID";

/// Silence after `session/prompt` with no inbound traffic and no outstanding
/// client work. Grok stays mute instead of erroring when `terminal` is missing.
pub const ACP_TURN_SILENCE: Duration = Duration::from_secs(60);
const TURN_TIMEOUT_ENV: &str = "DEVBOULE_ACP_TURN_TIMEOUT_MS";
const MAX_ACP_PERMISSION_LINE_BYTES: usize = 256 * 1024;
const ACP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

type AcpModeResponses = Arc<Mutex<HashMap<u64, Sender<Result<(), String>>>>>;

fn turn_silence() -> Duration {
    std::env::var(TURN_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .map(Duration::from_millis)
        .filter(|duration| !duration.is_zero())
        .unwrap_or(ACP_TURN_SILENCE)
}

fn advertised_initialize_params() -> Result<serde_json::Value, WireError> {
    let request = InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(
            ClientCapabilities::new()
                .fs(FileSystemCapabilities::new()
                    .read_text_file(true)
                    .write_text_file(true))
                .terminal(true),
        )
        .client_info(Implementation::new("devboule", env!("CARGO_PKG_VERSION")));
    serde_json::to_value(request).map_err(|error| {
        WireError::new(
            ErrorCode::Internal,
            format!("Could not encode ACP initialize request: {error}"),
        )
    })
}

#[derive(Clone, Copy)]
enum PromptPhase {
    Idle,
    Live(u64),
    Abandoned(u64),
}

struct TurnWatch {
    silence: Duration,
    last_activity: Mutex<Instant>,
    prompt: Mutex<PromptPhase>,
    client_work: std::sync::atomic::AtomicU64,
    stop: AtomicBool,
    runtime: Mutex<Option<Weak<SessionRuntime>>>,
    cancel: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    broker: Mutex<Option<Weak<PermissionBroker>>>,
}

impl TurnWatch {
    fn new() -> Arc<Self> {
        let watch = Arc::new(Self {
            silence: turn_silence(),
            last_activity: Mutex::new(Instant::now()),
            prompt: Mutex::new(PromptPhase::Idle),
            client_work: std::sync::atomic::AtomicU64::new(0),
            stop: AtomicBool::new(false),
            runtime: Mutex::new(None),
            cancel: Mutex::new(None),
            broker: Mutex::new(None),
        });
        let thread_watch = Arc::downgrade(&watch);
        let _ = std::thread::Builder::new()
            .name("acp-turn-timeout".to_string())
            .spawn(move || loop {
                let Some(watch) = thread_watch.upgrade() else {
                    return;
                };
                if watch.stop.load(Ordering::Acquire) {
                    return;
                }
                watch.tick();
                drop(watch);
                std::thread::sleep(Duration::from_millis(50));
            });
        watch
    }

    fn set_cancel(&self, cancel: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut slot) = self.cancel.lock() {
            *slot = Some(cancel);
        }
    }

    fn bind_runtime(&self, runtime: &Arc<SessionRuntime>) {
        if let Ok(mut slot) = self.runtime.lock() {
            *slot = Some(Arc::downgrade(runtime));
        }
    }

    fn bind_broker(&self, broker: &Arc<PermissionBroker>) {
        if let Ok(mut slot) = self.broker.lock() {
            *slot = Some(Arc::downgrade(broker));
        }
    }

    fn note_activity(&self) {
        if let Ok(mut last) = self.last_activity.lock() {
            *last = Instant::now();
        }
    }

    fn begin_client_work(&self) {
        self.client_work.fetch_add(1, Ordering::AcqRel);
        self.note_activity();
    }

    fn end_client_work(&self) {
        self.client_work
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
        self.note_activity();
    }

    fn start_prompt(&self, id: u64) {
        if let Ok(mut prompt) = self.prompt.lock() {
            *prompt = PromptPhase::Live(id);
        }
        self.note_activity();
    }

    fn finish_prompt(&self, id: u64) -> bool {
        let Ok(mut prompt) = self.prompt.lock() else {
            return false;
        };
        match *prompt {
            PromptPhase::Live(current) if current == id => {
                *prompt = PromptPhase::Idle;
                true
            }
            PromptPhase::Abandoned(current) if current == id => false,
            _ => false,
        }
    }

    fn prompt_is_live(&self) -> bool {
        matches!(
            self.prompt.lock().ok().as_deref(),
            Some(PromptPhase::Live(_))
        )
    }

    fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
    }

    fn abandon_live_prompt(&self) -> Option<u64> {
        let Ok(mut prompt) = self.prompt.lock() else {
            return None;
        };
        match *prompt {
            PromptPhase::Live(id) => {
                *prompt = PromptPhase::Abandoned(id);
                Some(id)
            }
            _ => None,
        }
    }

    fn tick(&self) {
        if self.stop.load(Ordering::Acquire) {
            return;
        }
        if self.client_work.load(Ordering::Acquire) != 0 {
            return;
        }
        let pending_permission = self
            .broker
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(Weak::upgrade))
            .map(|broker| broker.pending_len() > 0)
            .unwrap_or(false);
        if pending_permission {
            return;
        }
        let idle = self
            .last_activity
            .lock()
            .ok()
            .map(|last| last.elapsed() >= self.silence)
            .unwrap_or(false);
        if !idle {
            return;
        }
        if self.client_work.load(Ordering::Acquire) != 0 {
            return;
        }
        if self
            .broker
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(Weak::upgrade))
            .map(|broker| broker.pending_len() > 0)
            .unwrap_or(false)
        {
            return;
        }
        let Some(prompt_id) = self.abandon_live_prompt() else {
            return;
        };
        if let Ok(cancel) = self.cancel.lock() {
            if let Some(cancel) = cancel.as_ref() {
                cancel();
            }
        }
        if let Some(runtime) = self
            .runtime
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(Weak::upgrade))
        {
            let _ = runtime.publish_agent_event(
                SessionEvent::AgentError {
                    message: format!(
                        "ACP prompt {prompt_id} stayed silent for {}s and was cancelled.",
                        self.silence.as_secs().max(1)
                    ),
                },
                None,
            );
        }
    }
}

impl Drop for TurnWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl PermissionBroker {
    fn new(stdin: Arc<Mutex<Option<ChildStdin>>>) -> Arc<Self> {
        Self::with_sender(Arc::new(move |id, result| {
            let mut bytes = serde_json::to_vec(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            bytes.push(b'\n');
            write_child_stdin(&stdin, &bytes, "ACP")
        }))
    }
}

/// Resolve a direct executable plus argument vector. The JSON-array override
/// is intentional: it has no shell grammar and therefore remains correct for
/// executable paths containing spaces.
pub(super) fn resolve_command(_paths: &RuntimePaths) -> Result<PtyCommand, WireError> {
    let cwd = std::env::current_dir().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not determine agent working directory: {error}"),
        )
    })?;
    let (provider_id, mut argv): (Option<String>, Vec<String>) = match std::env::var(COMMAND_ENV) {
        Ok(argv) => {
            let provider_id = std::env::var(COMMAND_PROVIDER_ENV)
                .ok()
                .filter(|id| !id.trim().is_empty());
            let argv = serde_json::from_str(&argv).map_err(|error| {
                WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("{COMMAND_ENV} must be a non-empty JSON string array: {error}"),
                )
            })?;
            (provider_id, argv)
        }
        Err(_) => {
            let Some(agent) = crate::provider_catalog::first_acp_available() else {
                return Err(WireError::new(
                    ErrorCode::Io,
                    format!(
                        "No ACP-capable agent was found on PATH. Set {COMMAND_ENV} to a non-empty JSON string array to choose an ACP command explicitly."
                    ),
                ));
            };
            (
                Some(agent.id.to_string()),
                agent
                    .acp_command
                    .expect("an ACP-capable catalog entry has an ACP command"),
            )
        }
    };
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("{COMMAND_ENV} must contain an executable."),
        ));
    }
    let program = argv.remove(0);
    let mut command = PtyCommand::new(program, argv, cwd, Vec::new());
    if let Some(provider_id) = provider_id {
        command = command.with_provider_id(provider_id);
    }
    Ok(command)
}

/// Resolve a specific ACP catalog agent by id. Local PATH agents and
/// registry npx-wrapper rows are both accepted; npx-wrapper is only
/// reachable through this explicit-id path.
pub(super) fn resolve_named(id: &str, paths: &RuntimePaths) -> Result<PtyCommand, WireError> {
    let cwd = std::env::current_dir().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not determine agent working directory: {error}"),
        )
    })?;
    // Direct-command test providers have no catalog entry. Keep this narrow:
    // only the exact provider identity paired with DEVBOULE_ACP_COMMAND may
    // use the override, preserving the normal named catalog resolution path.
    if std::env::var(COMMAND_ENV).is_ok()
        && std::env::var(COMMAND_PROVIDER_ENV).ok().as_deref() == Some(id)
    {
        let command = resolve_command(paths)?;
        if command.provider_id.as_deref() == Some(id) {
            return Ok(command);
        }
    }
    let Some(agent) = crate::provider_catalog::find_in_catalog(
        id,
        &crate::registry::CdnRegistryFetch,
        &paths.dir,
    ) else {
        return Err(WireError::new(
            ErrorCode::Io,
            format!("ACP agent '{id}' was not found on PATH or in the ACP registry."),
        ));
    };
    let Some(mut argv) = agent.acp_command else {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("Provider '{id}' is not an ACP agent."),
        ));
    };
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("ACP agent '{id}' resolved to an empty command."),
        ));
    }
    let program = argv.remove(0);
    Ok(PtyCommand::new(program, argv, cwd, Vec::new()).with_provider_id(id.to_string()))
}

/// Spawn the ACP peer directly, complete initialize + session/new, and return
/// adapters that the ordinary session machinery can own.
pub(super) fn spawn_process(
    state: &Arc<ServerState>,
    command: PtyCommand,
    mcp: Option<McpLaunchConfig>,
    requested_mode: Option<String>,
) -> Result<SpawnedSession, WireError> {
    spawn_process_with_load(state, command, None, mcp, requested_mode)
}

pub(super) fn spawn_process_resuming(
    state: &Arc<ServerState>,
    command: PtyCommand,
    peer_session_id: String,
    mcp: Option<McpLaunchConfig>,
) -> Result<SpawnedSession, WireError> {
    spawn_process_with_load(state, command, Some(peer_session_id), mcp, None)
}

fn spawn_process_with_load(
    state: &Arc<ServerState>,
    command: PtyCommand,
    load_session_id: Option<String>,
    mcp: Option<McpLaunchConfig>,
    requested_mode: Option<String>,
) -> Result<SpawnedSession, WireError> {
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
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW prevents a desktop app from flashing a console for
        // every ACP agent process.
        process.creation_flags(0x0800_0000);
    }
    let mut child = process.spawn().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not start ACP agent {}: {error}", command.program),
        )
    })?;

    #[cfg(windows)]
    let (process_job, os_handle) = {
        use std::os::windows::io::AsRawHandle;
        let process_job = JobObject::new().map_err(|error| {
            terminate_process(&mut child);
            WireError::new(
                ErrorCode::Io,
                format!("Could not create the ACP process job: {error}"),
            )
        })?;
        let handle = child.as_raw_handle();
        if let Err(error) = state
            .process_job
            .assign(handle)
            .and_then(|()| process_job.assign(handle))
        {
            terminate_process(&mut child);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not contain the ACP agent process: {error}"),
            ));
        }
        let os_handle = match ProcessHandle::duplicate(handle) {
            Ok(duplicated) => Some(duplicated),
            Err(error) => {
                eprintln!("could not duplicate ACP process handle for OS liveness: {error}");
                None
            }
        };
        (process_job, os_handle)
    };

    #[cfg(not(windows))]
    let process_job = JobObject::new().map_err(|error| {
        terminate_process(&mut child);
        WireError::new(
            ErrorCode::Io,
            format!("Could not create the ACP process job: {error}"),
        )
    })?;
    #[cfg(not(windows))]
    let os_handle = None;

    let stdin = child.stdin.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "ACP agent did not provide stdin.")
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "ACP agent did not provide stdout.")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "ACP agent did not provide stderr.")
    })?;

    let process = Arc::new(Mutex::new(child));
    let host = AcpHost::new(
        command.cwd.clone(),
        state.sessions.runtime_dir().to_path_buf(),
        Arc::clone(&state.process_job),
    );
    let transport = Arc::new(AcpTransport::new(stdin, Arc::clone(&host)));
    transport.bind_turn();
    let mut stderr_source = match AcpStderr::start(stderr) {
        Ok(source) => source,
        Err(error) => {
            if let Ok(mut process) = process.lock() {
                terminate_process(&mut process);
            }
            drop(process_job);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not drain ACP stderr: {error}"),
            ));
        }
    };
    let mut reader = BufReader::new(stdout);
    let (deferred, handshake, peer_session_id, agent_version) = match handshake(
        &transport,
        &mut reader,
        &command.cwd,
        command.provider_id.clone(),
        load_session_id.as_deref(),
        mcp.as_ref(),
        requested_mode.as_deref(),
    ) {
        Ok(handshake) => handshake,
        Err(error) => {
            let mut killer = AcpKiller {
                process: Arc::clone(&process),
                transport: Arc::clone(&transport),
                permission_broker: Arc::clone(&transport.permission_broker),
                cancelled: Arc::new(AtomicBool::new(false)),
            };
            killer.kill();
            drop(killer);
            // AcpTransport owns the only stdin handle. Close it before waiting
            // for a peer that may require EOF to finish its shutdown path.
            drop(transport);
            if let Ok(mut process) = process.lock() {
                let _ = process.wait();
            }
            let stderr_lines = stderr_source.discard_and_join();
            drop(process_job);
            return Err(redact_handshake_error(error, &stderr_lines, mcp.as_ref()));
        }
    };
    let session_id = transport.session_id();
    transport.set_model_switch_shape(handshake.shape);
    transport.set_prompt_capabilities(handshake.prompt_capabilities);
    transport.seed_manifest_from_event(handshake.event.as_ref());
    let writer = AcpWriter {
        transport: Arc::clone(&transport),
        pending: Vec::new(),
    };
    let killer = AcpKiller {
        process: Arc::clone(&process),
        transport: Arc::clone(&transport),
        permission_broker: Arc::clone(&transport.permission_broker),
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    let reader_dispatch = AcpReader::new(
        transport.pending_ids(),
        transport.model_switch_ids(),
        transport.mode_switch_ids(),
        session_id,
        Arc::clone(&transport.permission_broker),
        Arc::clone(&transport.host),
        Arc::clone(&transport.turn),
        Some(Arc::clone(&transport)),
        deferred,
        command.provider_id.clone(),
        handshake.event,
    );
    Ok(SpawnedSession {
        process_job,
        master: None,
        killer: Box::new(killer),
        switcher: Some(Box::new(AcpSwitcher {
            transport: Arc::clone(&transport),
        })),
        child: Box::new(StdioWaitableChild { process }),
        writer: Arc::new(Mutex::new(Box::new(writer) as Box<dyn Write + Send>)),
        // The sibling is installed for every ACP session; whether a prompt
        // uses it is decided per prompt from the live negotiated capability.
        // An agent that never declared `promptCapabilities.image` keeps the
        // sibling present but unused, falling back to the path line.
        image_sink: Some(Arc::new(super::AcpPromptSink::new(&transport))),
        // The negotiated route above is the ACP one; the static route the
        // three other providers carry is not installed for this session.
        static_image_sink: None,
        reader: Box::new(reader),
        reader_dispatch: Some(Box::new(reader_dispatch)),
        stderr: Some(Box::new(stderr_source)),
        permission_broker: Some(Arc::clone(&transport.permission_broker)),
        os_handle,
        peer_session_id: Some(peer_session_id),
        agent_version,
    })
}

fn terminate_process(process: &mut Child) {
    let _ = process.kill();
    let _ = process.wait();
}

#[derive(Clone, Debug)]
enum SwitchRequest {
    Vendor {
        model_id: String,
        effort: Option<String>,
    },
    Config {
        config_id: String,
        value: String,
        control: ConfigControl,
    },
}

#[derive(Clone, Debug)]
enum AlternateSwitch {
    Request(SwitchRequest),
    Blocked(String),
}

#[derive(Clone, Copy, Debug)]
enum ConfigControl {
    Model,
    Effort,
}

#[derive(Clone, Debug)]
struct FollowupSwitch {
    request: SwitchRequest,
    alternate: Option<AlternateSwitch>,
    requested_model_id: Option<String>,
    requested_effort: Option<String>,
}

/// A pending switch carries the requested values and the one permitted
/// alternate surface. Errors are handled here, after the asynchronous reply;
/// there is no verb preference negotiated at handshake time.
#[derive(Clone, Debug)]
enum PendingSwitch {
    SetModel {
        model_id: String,
        effort: Option<String>,
        alternate: Option<AlternateSwitch>,
        followup: Option<FollowupSwitch>,
    },
    SetConfigOption {
        config_id: String,
        value: String,
        control: ConfigControl,
        requested_model_id: Option<String>,
        requested_effort: Option<String>,
        alternate: Option<AlternateSwitch>,
        followup: Option<FollowupSwitch>,
    },
}

pub(super) struct AcpTransport {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    permission_broker: Arc<PermissionBroker>,
    host: Arc<AcpHost>,
    turn: Arc<TurnWatch>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashSet<u64>>>,
    model_switches: Arc<Mutex<HashMap<u64, PendingSwitch>>>,
    mode_switches: AcpModeResponses,
    remote_modes: AtomicBool,
    session_id: Mutex<Option<String>>,
    current_model_id: Mutex<Option<String>>,
    current_effort: Mutex<Option<String>>,
    last_manifest: Mutex<Option<SessionEvent>>,
    model_switch_shape: Mutex<Option<ModelSwitchShape>>,
    /// Prompt content types the agent declared in `initialize`. Session-scoped
    /// negotiated state, kept next to the other handshake results
    /// (`model_switch_shape`, `remote_modes`). Re-derived on a `session/load`
    /// handshake like the rest of the negotiated state, never guessed.
    prompt_capabilities: Mutex<PromptCapabilities>,
}

/// The sink's write halves share one `session/prompt` sender: text-only
/// ([`AcpWriter`]) and text-plus-image blocks (the structured sibling).
/// Kept on the transport so both halves read the same session id and feed
/// the same turn watch.
impl AcpTransport {
    fn new(stdin: ChildStdin, host: Arc<AcpHost>) -> Self {
        let stdin = Arc::new(Mutex::new(Some(stdin)));
        Self {
            permission_broker: PermissionBroker::new(Arc::clone(&stdin)),
            host,
            turn: TurnWatch::new(),
            stdin,
            next_id: AtomicU64::new(1),
            pending: Arc::new(Mutex::new(HashSet::new())),
            model_switches: Arc::new(Mutex::new(HashMap::new())),
            mode_switches: Arc::new(Mutex::new(HashMap::new())),
            remote_modes: AtomicBool::new(false),
            session_id: Mutex::new(None),
            current_model_id: Mutex::new(None),
            current_effort: Mutex::new(None),
            last_manifest: Mutex::new(None),
            model_switch_shape: Mutex::new(None),
            prompt_capabilities: Mutex::new(PromptCapabilities::default()),
        }
    }

    fn send_result(
        &self,
        id: serde_json::Value,
        result: Result<serde_json::Value, RpcError>,
    ) -> io::Result<()> {
        let value = match result {
            Ok(result) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }),
            Err(error) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": error.to_json(),
            }),
        };
        self.send_line(&value)
    }

    fn send_line(&self, value: &serde_json::Value) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(value)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        bytes.push(b'\n');
        write_child_stdin(&self.stdin, &bytes, "ACP")
    }

    fn close_stdin(&self) {
        if let Ok(mut stdin) = self.stdin.lock() {
            *stdin = None;
        }
    }

    /// Issues one `session/prompt` request on this transport's stdin and starts
    /// the turn watch on its id. [`AcpWriter`] sends a text-only block through
    /// here; the structured sibling ([`super::AcpPromptSink`]) sends text plus
    /// image blocks through [`AcpTransport::send_structured_prompt`] with the
    /// same allocator and pending table, so both halves draw request ids from
    /// one sequence.
    fn send_prompt(&self, prompt: Vec<serde_json::Value>) -> io::Result<u64> {
        let id = send_prompt_on(self, prompt)?;
        self.turn.start_prompt(id);
        Ok(id)
    }

    /// The structured sibling's write half ([`super::AcpPromptSink`]): text
    /// plus image blocks through the same sender, id sequence, pending
    /// table, session id and turn watch.
    pub(super) fn send_structured_prompt(&self, prompt: Vec<serde_json::Value>) -> io::Result<u64> {
        self.send_prompt(prompt)
    }

    fn request(&self, method: &str, params: serde_json::Value) -> io::Result<u64> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending
            .lock()
            .map_err(|_| io::Error::other("ACP pending-id lock poisoned"))?
            .insert(id);
        if let Err(error) = self.send_line(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        })) {
            self.remove_pending_id(id);
            return Err(error);
        }
        Ok(id)
    }

    fn notify(&self, method: &str, params: serde_json::Value) -> io::Result<()> {
        self.send_line(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    fn response_seen(&self, id: u64) -> bool {
        self.pending
            .lock()
            .map(|mut pending| pending.remove(&id))
            .unwrap_or(false)
    }

    fn mode_switch_ids(&self) -> AcpModeResponses {
        Arc::clone(&self.mode_switches)
    }

    fn set_remote_modes(&self, supported: bool) {
        self.remote_modes.store(supported, Ordering::Release);
    }

    fn has_remote_modes(&self) -> bool {
        self.remote_modes.load(Ordering::Acquire)
    }
}

/// One `session/prompt` request on the transport's own handles: the shared
/// write half behind both [`AcpTransport::send_prompt`] (text-only) and
/// [`AcpTransport::send_structured_prompt`] (text plus image blocks). It
/// takes `&self` so there is nothing new to share with the sibling — both
/// halves already hold the transport — and so the two cannot drift into
/// different envelopes, id sequences, or pending tables.
fn send_prompt_on(transport: &AcpTransport, prompt: Vec<serde_json::Value>) -> io::Result<u64> {
    let id = transport.next_id.fetch_add(1, Ordering::Relaxed);
    transport
        .pending
        .lock()
        .map_err(|_| io::Error::other("ACP pending-id lock poisoned"))?
        .insert(id);
    let frame = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/prompt",
        "params": {
            "sessionId": transport.session_id(),
            "prompt": prompt,
        },
    });
    let mut bytes = serde_json::to_vec(&frame)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    if let Err(error) = super::write_child_stdin(&transport.stdin, &bytes, "ACP") {
        // Same poison handling as `request`'s failure path: a poisoned table
        // still gives up its guard, so the id never leaks into `pending`.
        match transport.pending.lock() {
            Ok(mut pending) => {
                pending.remove(&id);
            }
            Err(poisoned) => {
                poisoned.into_inner().remove(&id);
            }
        }
        return Err(error);
    }
    Ok(id)
}

impl AcpTransport {
    fn remove_pending_id(&self, id: u64) {
        match self.pending.lock() {
            Ok(mut pending) => {
                pending.remove(&id);
            }
            Err(poisoned) => {
                poisoned.into_inner().remove(&id);
            }
        }
    }

    fn remove_model_switch(&self, id: u64) {
        match self.model_switches.lock() {
            Ok(mut switches) => {
                switches.remove(&id);
            }
            Err(poisoned) => {
                poisoned.into_inner().remove(&id);
            }
        }
    }

    fn pending_ids(&self) -> Arc<Mutex<HashSet<u64>>> {
        Arc::clone(&self.pending)
    }

    fn model_switch_ids(&self) -> Arc<Mutex<HashMap<u64, PendingSwitch>>> {
        Arc::clone(&self.model_switches)
    }

    fn set_model_switch_shape(&self, shape: Option<ModelSwitchShape>) {
        let mut shape_slot = match self.model_switch_shape.lock() {
            Ok(shape_slot) => shape_slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        *shape_slot = shape;
    }

    fn model_switch_shape(&self) -> Option<ModelSwitchShape> {
        match self.model_switch_shape.lock() {
            Ok(shape) => shape.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn set_prompt_capabilities(&self, capabilities: PromptCapabilities) {
        let mut slot = match self.prompt_capabilities.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = capabilities;
    }

    /// The read side for the structured prompt sink: the per-prompt delivery
    /// decision reads the live negotiated verdict, so a `session/load`
    /// handshake that re-derives it is honoured without a respawn.
    pub(super) fn prompt_capabilities(&self) -> PromptCapabilities {
        match self.prompt_capabilities.lock() {
            Ok(capabilities) => *capabilities,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    fn set_session_id(&self, session_id: String) {
        let mut current = match self.session_id.lock() {
            Ok(current) => current,
            Err(poisoned) => poisoned.into_inner(),
        };
        *current = Some(session_id);
    }

    fn session_id(&self) -> String {
        match self.session_id.lock() {
            Ok(value) => value.clone().unwrap_or_default(),
            Err(poisoned) => poisoned.into_inner().clone().unwrap_or_default(),
        }
    }

    fn current_model_id(&self) -> Option<String> {
        match self.current_model_id.lock() {
            Ok(value) => value.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn update_current_model_id(&self, model_id: Option<String>) {
        if let Some(model_id) = model_id.filter(|model_id| !model_id.is_empty()) {
            let mut current = match self.current_model_id.lock() {
                Ok(current) => current,
                Err(poisoned) => poisoned.into_inner(),
            };
            *current = Some(model_id);
        }
    }

    fn current_effort(&self) -> Option<String> {
        match self.current_effort.lock() {
            Ok(effort) => effort.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn update_current_effort(&self, effort: Option<String>) {
        let mut current = match self.current_effort.lock() {
            Ok(current) => current,
            Err(poisoned) => poisoned.into_inner(),
        };
        *current = effort.filter(|effort| !effort.is_empty());
    }

    fn update_manifest_from_sessions_changed(
        &self,
        model_id: Option<String>,
        effort: Option<String>,
    ) {
        if let Some(model_id) = model_id {
            self.update_current_model_id(Some(model_id));
        }
        // `_x.ai/sessions/changed.reasoningEffort` is the session's ACTUAL live
        // effort, not a catalog default (measured: it reports `low`/`high` as
        // the user set them, whereas `models/update` reports the model's default
        // regardless). It is therefore authoritative and MUST update the tracked
        // effort — this is what makes `override_manifest_effort` reflect a change
        // the provider made on its own side. Do not "protect" the tracked effort
        // from this source: that would hide a real runtime change and violate
        // §4.3.4 (show the runtime-confirmed value, not the last click).
        if effort.is_some() {
            self.update_current_effort(effort);
        }
    }

    fn seed_manifest_from_event(&self, event: Option<&SessionEvent>) {
        if let Some(SessionEvent::SessionManifest {
            current_model_id,
            models,
            ..
        }) = event
        {
            self.update_current_model_id(current_model_id.clone());
            let effort = current_model_id.as_ref().and_then(|model_id| {
                models
                    .iter()
                    .find(|model| model.model_id == *model_id)
                    .and_then(|model| model.current_effort.clone())
            });
            self.update_current_effort(effort);
            let mut last_manifest = match self.last_manifest.lock() {
                Ok(last_manifest) => last_manifest,
                Err(poisoned) => poisoned.into_inner(),
            };
            *last_manifest = event.cloned();
        }
    }

    fn remember_manifest(&self, event: &SessionEvent) {
        if let SessionEvent::SessionManifest {
            current_model_id, ..
        } = event
        {
            self.update_current_model_id(current_model_id.clone());
            let mut last_manifest = match self.last_manifest.lock() {
                Ok(last_manifest) => last_manifest,
                Err(poisoned) => poisoned.into_inner(),
            };
            *last_manifest = Some(event.clone());
        }
    }

    fn remember_mode(&self, mode_id: &str) {
        let Some(SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes: Some(mut modes),
        }) = self.last_manifest()
        else {
            return;
        };
        modes.current_mode_id = mode_id.to_string();
        self.remember_manifest(&SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes: Some(modes),
        });
    }

    fn override_manifest_effort(&self, event: SessionEvent) -> SessionEvent {
        let Some(effort) = self.current_effort() else {
            return event;
        };
        let SessionEvent::SessionManifest {
            provider_id,
            current_model_id: Some(current_model_id),
            models,
            modes,
        } = event
        else {
            return event;
        };
        let models = models
            .into_iter()
            .map(|mut model| {
                if model.model_id == current_model_id {
                    model.current_effort = Some(effort.clone());
                }
                model
            })
            .collect();
        SessionEvent::SessionManifest {
            provider_id,
            current_model_id: Some(current_model_id),
            models,
            modes,
        }
    }

    fn last_manifest(&self) -> Option<SessionEvent> {
        match self.last_manifest.lock() {
            Ok(manifest) => manifest.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn default_effort_for_model(&self, model_id: &str) -> Option<String> {
        let SessionEvent::SessionManifest { models, .. } = self.last_manifest()? else {
            return None;
        };
        let model = models.iter().find(|model| model.model_id == model_id)?;
        model.current_effort.clone().or_else(|| {
            model.efforts.as_ref().and_then(|efforts| {
                efforts
                    .iter()
                    .find(|effort| effort.default == Some(true))
                    .or_else(|| efforts.first())
                    .map(|effort| effort.id.clone())
            })
        })
    }

    fn request_set_model(
        &self,
        model_id: String,
        effort: Option<String>,
        alternate: Option<AlternateSwitch>,
        followup: Option<FollowupSwitch>,
    ) -> io::Result<u64> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending
            .lock()
            .map_err(|_| io::Error::other("ACP pending-id lock poisoned"))?
            .insert(id);
        self.model_switches
            .lock()
            .map_err(|_| io::Error::other("ACP model-switch lock poisoned"))?
            .insert(
                id,
                PendingSwitch::SetModel {
                    model_id: model_id.clone(),
                    effort: effort.clone(),
                    alternate,
                    followup,
                },
            );
        let mut params = serde_json::json!({
            "sessionId": self.session_id(),
            "modelId": model_id,
        });
        if let Some(effort) = &effort {
            params["_meta"] = serde_json::json!({ "reasoningEffort": effort });
        }
        if let Err(error) = self.send_line(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/set_model",
            "params": params,
        })) {
            self.remove_pending_id(id);
            self.remove_model_switch(id);
            return Err(error);
        }
        Ok(id)
    }

    /// ACP v1 model/effort switch. Measured on
    /// `@agentclientprotocol/claude-agent-acp@0.76.0`: the value is a
    /// PLAIN STRING (`value: "haiku"`); the SDK's `{type:"id",value:…}`
    /// wrapper is rejected by the agent's zod schema, and the reply carries
    /// the full updated `configOptions`.
    #[allow(clippy::too_many_arguments)]
    fn request_set_config_option(
        &self,
        config_id: &str,
        value: &str,
        control: ConfigControl,
        requested_model_id: Option<String>,
        requested_effort: Option<String>,
        alternate: Option<AlternateSwitch>,
        followup: Option<FollowupSwitch>,
    ) -> io::Result<u64> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending
            .lock()
            .map_err(|_| io::Error::other("ACP pending-id lock poisoned"))?
            .insert(id);
        self.model_switches
            .lock()
            .map_err(|_| io::Error::other("ACP model-switch lock poisoned"))?
            .insert(
                id,
                PendingSwitch::SetConfigOption {
                    config_id: config_id.to_string(),
                    value: value.to_string(),
                    control,
                    requested_model_id,
                    requested_effort,
                    alternate,
                    followup,
                },
            );
        if let Err(error) = self.send_line(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/set_config_option",
            "params": {
                "sessionId": self.session_id(),
                "configId": config_id,
                "value": value,
            },
        })) {
            self.remove_pending_id(id);
            self.remove_model_switch(id);
            return Err(error);
        }
        Ok(id)
    }

    fn request_set_mode(&self, mode_id: &str) -> Result<(), WireError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| WireError::new(ErrorCode::Io, "ACP pending-id lock poisoned"))?
            .insert(id);
        self.mode_switches
            .lock()
            .map_err(|_| WireError::new(ErrorCode::Io, "ACP mode-switch lock poisoned"))?
            .insert(id, sender);
        if let Err(error) = self.send_line(&session_set_mode_frame(id, &self.session_id(), mode_id))
        {
            let _ = self.pending.lock().map(|mut pending| pending.remove(&id));
            let _ = self
                .mode_switches
                .lock()
                .map(|mut switches| switches.remove(&id));
            return Err(acp_io_error(error));
        }
        match receiver.recv_timeout(ACP_RESPONSE_TIMEOUT) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(WireError::new(ErrorCode::InvalidRequest, message)),
            Err(error) => {
                let _ = self.pending.lock().map(|mut pending| pending.remove(&id));
                let _ = self
                    .mode_switches
                    .lock()
                    .map(|mut switches| switches.remove(&id));
                Err(WireError::new(
                    ErrorCode::Io,
                    format!("ACP session/set_mode response timed out: {error}"),
                ))
            }
        }
    }

    fn cancel(&self) {
        let session_id = self.session_id();
        if session_id.is_empty() {
            return;
        }
        let _ = self.notify(
            "session/cancel",
            serde_json::json!({ "sessionId": session_id }),
        );
    }

    fn bind_turn(self: &Arc<Self>) {
        let cancel_transport = Arc::downgrade(self);
        self.turn.set_cancel(Arc::new(move || {
            if let Some(transport) = cancel_transport.upgrade() {
                transport.cancel();
            }
        }));
        self.turn.bind_broker(&self.permission_broker);
    }
}

fn session_set_mode_frame(id: u64, session_id: &str, mode_id: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/set_mode",
        "params": {
            "sessionId": session_id,
            "modeId": mode_id,
        },
    })
}

struct AcpSwitcher {
    transport: Arc<AcpTransport>,
}

impl ModelSwitcher for AcpSwitcher {
    fn set_model(&self, model_id: Option<&str>, effort: Option<&str>) -> Result<(), WireError> {
        let shape = self.transport.model_switch_shape().ok_or_else(|| {
            WireError::new(
                ErrorCode::InvalidRequest,
                "This agent's session carries no declared model or effort switch surface; \
                 the switch verb is unknown. Reconnect the session to re-read the agent's \
                 controls.",
            )
        })?;
        let requested_model = model_id
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let requested_effort = effort.filter(|value| !value.is_empty()).map(str::to_string);

        if let Some(model_id) = requested_model {
            return self.set_requested_model(shape, model_id, requested_effort);
        }
        if let Some(effort) = requested_effort {
            return self.set_requested_effort(shape, effort);
        }
        let current_model = self.transport.current_model_id().ok_or_else(|| {
            WireError::new(
                ErrorCode::InvalidRequest,
                "ACP provider has not reported a current model.",
            )
        })?;
        self.set_requested_model(shape, current_model, None)
    }

    fn set_mode(&self, mode_id: &str) -> Result<(), WireError> {
        let result = if self.transport.has_remote_modes() {
            self.transport.request_set_mode(mode_id)
        } else {
            Ok(())
        };
        if result.is_ok() {
            self.transport.remember_mode(mode_id);
        }
        result
    }

    fn clone_switcher(&self) -> Box<dyn ModelSwitcher> {
        Box::new(Self {
            transport: Arc::clone(&self.transport),
        })
    }
}

impl AcpSwitcher {
    fn set_requested_model(
        &self,
        shape: ModelSwitchShape,
        model_id: String,
        effort: Option<String>,
    ) -> Result<(), WireError> {
        let effort_uses_config = shape.effort.config.is_some();
        let followup = if effort_uses_config {
            effort.as_ref().and_then(|effort| {
                shape.effort.config.as_ref().map(|config| FollowupSwitch {
                    request: SwitchRequest::Config {
                        config_id: config.id.clone(),
                        value: effort.clone(),
                        control: ConfigControl::Effort,
                    },
                    alternate: self.vendor_effort_alternate(
                        &shape,
                        &model_id,
                        effort,
                        "the requested effort",
                    ),
                    requested_model_id: Some(model_id.clone()),
                    requested_effort: Some(effort.clone()),
                })
            })
        } else {
            None
        };
        match shape.model.config.as_ref() {
            Some(config) => {
                let alternate = self.vendor_model_alternate(&shape, &model_id, effort.clone());
                self.transport
                    .request_set_config_option(
                        &config.id,
                        &model_id,
                        ConfigControl::Model,
                        Some(model_id.clone()),
                        effort.clone(),
                        alternate,
                        followup,
                    )
                    .map_err(acp_io_error)?;
            }
            None => {
                // The old vendor catalog sometimes carries a per-model
                // default effort. Preserve that workaround only when the
                // agent declared no config-option effort surface: once an
                // effort option is declared, it is the spec-stable control
                // and a model switch must not invent a value on its behalf.
                let vendor_effort = if effort_uses_config {
                    None
                } else {
                    effort
                        .clone()
                        .or_else(|| self.transport.default_effort_for_model(&model_id))
                };
                let alternate = self.config_model_alternate(&shape, &model_id);
                self.transport
                    .request_set_model(model_id, vendor_effort, alternate, followup)
                    .map_err(acp_io_error)?;
            }
        }
        Ok(())
    }

    fn set_requested_effort(
        &self,
        shape: ModelSwitchShape,
        effort: String,
    ) -> Result<(), WireError> {
        let current_model = self.transport.current_model_id();
        match shape.effort.config.as_ref() {
            Some(config) => {
                let alternate = match current_model.as_deref() {
                    Some(model_id) => self.vendor_effort_alternate(
                        &shape,
                        model_id,
                        &effort,
                        "the requested effort",
                    ),
                    None if shape.effort.vendor.is_some() => Some(AlternateSwitch::Blocked(
                        "the vendor fallback needs the agent's current model, but the \
                         handshake did not report one"
                            .to_string(),
                    )),
                    None => None,
                };
                self.transport
                    .request_set_config_option(
                        &config.id,
                        &effort,
                        ConfigControl::Effort,
                        current_model,
                        Some(effort.clone()),
                        alternate,
                        None,
                    )
                    .map_err(acp_io_error)?;
            }
            None => {
                let model_id = current_model.ok_or_else(|| {
                    WireError::new(
                        ErrorCode::InvalidRequest,
                        "Cannot change effort before the provider reports its current model.",
                    )
                })?;
                let alternate = self.config_effort_alternate(&shape, &effort);
                self.transport
                    .request_set_model(model_id, Some(effort), alternate, None)
                    .map_err(acp_io_error)?;
            }
        }
        Ok(())
    }

    fn vendor_model_alternate(
        &self,
        shape: &ModelSwitchShape,
        model_id: &str,
        effort: Option<String>,
    ) -> Option<AlternateSwitch> {
        let vendor = shape.model.vendor.as_ref()?;
        if !vendor.values.iter().any(|value| value == model_id) {
            return Some(AlternateSwitch::Blocked(format!(
                "not retrying on the vendor model surface: model `{model_id}` is not one of its declared model ids"
            )));
        }
        if let Some(effort) = &effort {
            let values = self.vendor_effort_values(shape, model_id);
            if !values.iter().any(|value| value == effort) {
                return Some(AlternateSwitch::Blocked(format!(
                    "not retrying on the vendor effort surface: effort `{effort}` is not declared for model `{model_id}`"
                )));
            }
        }
        Some(AlternateSwitch::Request(SwitchRequest::Vendor {
            model_id: model_id.to_string(),
            effort,
        }))
    }

    fn config_model_alternate(
        &self,
        shape: &ModelSwitchShape,
        model_id: &str,
    ) -> Option<AlternateSwitch> {
        let config = shape.model.config.as_ref()?;
        if !config.values.iter().any(|value| value == model_id) {
            return Some(AlternateSwitch::Blocked(format!(
                "not retrying on the config-option model surface: model `{model_id}` is not one of option `{}`'s declared values",
                config.id
            )));
        }
        Some(AlternateSwitch::Request(SwitchRequest::Config {
            config_id: config.id.clone(),
            value: model_id.to_string(),
            control: ConfigControl::Model,
        }))
    }

    fn vendor_effort_alternate(
        &self,
        shape: &ModelSwitchShape,
        model_id: &str,
        effort: &str,
        label: &str,
    ) -> Option<AlternateSwitch> {
        let vendor = shape.effort.vendor.as_ref()?;
        if !vendor.values.iter().any(|value| value == effort) {
            return Some(AlternateSwitch::Blocked(format!(
                "not retrying on the vendor effort surface: {label} `{effort}` is not declared"
            )));
        }
        if !vendor
            .values_by_model
            .iter()
            .any(|(model, values)| model == model_id && values.iter().any(|value| value == effort))
        {
            return Some(AlternateSwitch::Blocked(format!(
                "not retrying on the vendor effort surface: effort `{effort}` is not declared for model `{model_id}`"
            )));
        }
        Some(AlternateSwitch::Request(SwitchRequest::Vendor {
            model_id: model_id.to_string(),
            effort: Some(effort.to_string()),
        }))
    }

    fn config_effort_alternate(
        &self,
        shape: &ModelSwitchShape,
        effort: &str,
    ) -> Option<AlternateSwitch> {
        let config = shape.effort.config.as_ref()?;
        if !config.values.iter().any(|value| value == effort) {
            return Some(AlternateSwitch::Blocked(format!(
                "not retrying on config option `{}`: effort `{effort}` is not one of its declared values",
                config.id
            )));
        }
        Some(AlternateSwitch::Request(SwitchRequest::Config {
            config_id: config.id.clone(),
            value: effort.to_string(),
            control: ConfigControl::Effort,
        }))
    }

    fn vendor_effort_values<'a>(
        &self,
        shape: &'a ModelSwitchShape,
        model_id: &str,
    ) -> &'a [String] {
        shape
            .effort
            .vendor
            .as_ref()
            .and_then(|vendor| {
                vendor
                    .values_by_model
                    .iter()
                    .find(|(model, _)| model == model_id)
                    .map(|(_, values)| values.as_slice())
            })
            .unwrap_or(&[])
    }
}

impl Drop for AcpTransport {
    fn drop(&mut self) {
        self.turn.shutdown();
        self.host.shutdown();
        self.close_stdin();
    }
}

type HandshakeResult = (
    Vec<serde_json::Value>,
    HandshakeManifest,
    String,
    Option<String>,
);

fn handshake(
    transport: &AcpTransport,
    reader: &mut BufReader<ChildStdout>,
    cwd: &std::path::Path,
    provider_id: Option<String>,
    load_session_id: Option<&str>,
    mcp: Option<&McpLaunchConfig>,
    requested_mode: Option<&str>,
) -> Result<HandshakeResult, WireError> {
    let mut deferred = Vec::new();
    let mcp_servers = mcp
        .map(|config| vec![config.acp_server_value()])
        .unwrap_or_default();
    let initialize_id = transport
        .request("initialize", advertised_initialize_params()?)
        .map_err(acp_io_error)?;
    let initialize = read_response(transport, reader, initialize_id, &mut deferred)?;
    let negotiated = initialize
        .get("result")
        .and_then(|result| result.get("protocolVersion"))
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            WireError::new(
                ErrorCode::Io,
                "ACP initialize returned no protocol version.",
            )
        })?;
    if negotiated != 1 {
        return Err(WireError::new(
            ErrorCode::Io,
            format!("ACP peer negotiated unsupported protocol version {negotiated}."),
        ));
    }
    let agent_version = initialize
        .get("result")
        .and_then(|result| result.get("agentInfo"))
        .and_then(|agent_info| agent_info.get("version"))
        .and_then(serde_json::Value::as_str)
        .and_then(crate::provider_catalog::cap_external_version);
    let (method, params) = match load_session_id {
        Some(session_id) => (
            "session/load",
            serde_json::json!({
                "sessionId": session_id,
                "cwd": cwd.to_string_lossy(),
                "mcpServers": mcp_servers
            }),
        ),
        None => (
            "session/new",
            serde_json::json!({
                "cwd": cwd.to_string_lossy(),
                "mcpServers": mcp_servers
            }),
        ),
    };
    let session_request_id = transport.request(method, params).map_err(acp_io_error)?;
    let session = read_response(transport, reader, session_request_id, &mut deferred)?;
    let session_id = match load_session_id {
        Some(session_id) if !session_id.is_empty() => session_id.to_string(),
        _ => session
            .get("result")
            .and_then(|result| result.get("sessionId"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                WireError::new(ErrorCode::Io, "ACP session/new returned no session id.")
            })?
            .to_string(),
    };
    transport.set_session_id(session_id.to_string());
    transport.host.set_session_id(session_id.to_string());
    let mut handshake = merge_handshake_manifest(
        initialize.get("result").unwrap_or(&serde_json::Value::Null),
        session.get("result").unwrap_or(&serde_json::Value::Null),
        provider_id,
    );
    let remote_modes = session
        .get("result")
        .map(has_standard_modes)
        .unwrap_or(false);
    transport.set_remote_modes(remote_modes);
    if let Some(requested_mode) = requested_mode {
        let current_mode = handshake.event.as_ref().and_then(|event| match event {
            SessionEvent::SessionManifest { modes, .. } => modes.as_ref(),
            _ => None,
        });
        let available = current_mode
            .map(|modes| {
                modes
                    .available_modes
                    .iter()
                    .any(|mode| mode.id == requested_mode)
            })
            .unwrap_or(false);
        if !available {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("ACP session mode '{requested_mode}' is not available."),
            ));
        }
        let needs_remote_switch = remote_modes
            && current_mode
                .map(|modes| modes.current_mode_id != requested_mode)
                .unwrap_or(false);
        if needs_remote_switch {
            let request_id = transport
                .request(
                    "session/set_mode",
                    serde_json::json!({
                        "sessionId": session_id,
                        "modeId": requested_mode,
                    }),
                )
                .map_err(acp_io_error)?;
            let _ = read_response(transport, reader, request_id, &mut deferred)?;
        }
        if let Some(SessionEvent::SessionManifest {
            modes: Some(modes), ..
        }) = &mut handshake.event
        {
            modes.current_mode_id = requested_mode.to_string();
        }
    }
    Ok((deferred, handshake, session_id.to_string(), agent_version))
}

fn read_response(
    transport: &AcpTransport,
    reader: &mut BufReader<ChildStdout>,
    expected_id: u64,
    deferred: &mut Vec<serde_json::Value>,
) -> Result<serde_json::Value, WireError> {
    loop {
        let mut line = String::new();
        let count = reader.read_line(&mut line).map_err(acp_io_error)?;
        if count == 0 {
            return Err(WireError::new(
                ErrorCode::Io,
                "ACP agent closed stdout during handshake.",
            ));
        }
        let line = line.trim_end_matches('\n').trim_end_matches('\r');
        let value = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("skipping malformed ACP handshake line: {error}");
                continue;
            }
        };
        if value.get("id").and_then(serde_json::Value::as_u64) != Some(expected_id) {
            deferred.push(value);
            continue;
        }
        if !transport.response_seen(expected_id) {
            eprintln!("skipping ACP response with an unknown id {expected_id}");
            continue;
        }
        if let Some(error) = value.get("error") {
            return Err(WireError::new(
                ErrorCode::Io,
                acp_request_error_message(error),
            ));
        }
        return Ok(value);
    }
}

/// Format a JSON-RPC error object for a user-facing message. Agents embed
/// structured payloads in the error object (qwen carries `authMethods`);
/// the string `message` field is what belongs in a chat banner. Without a
/// string message the code is reported bare — the object itself is never
/// serialized into the text.
fn acp_request_error_message(error: &serde_json::Value) -> String {
    let message = error.get("message").and_then(serde_json::Value::as_str);
    match (
        error.get("code").and_then(serde_json::Value::as_i64),
        message,
    ) {
        (Some(code), Some(message)) => format!("ACP request failed ({code}): {message}"),
        (None, Some(message)) => format!("ACP request failed: {message}"),
        // A structured payload (tokens, authMethods) belongs to provider
        // plumbing, never to a chat banner or the Settings health line, so
        // the object itself is never serialized here.
        (Some(code), None) => format!("ACP request failed ({code})."),
        (None, None) => "ACP request failed.".to_string(),
    }
}

fn acp_io_error(error: io::Error) -> WireError {
    WireError::new(ErrorCode::Io, format!("ACP stdio failed: {error}"))
}

fn redact_mcp_error(mut error: WireError, mcp: Option<&McpLaunchConfig>) -> WireError {
    if let Some(mcp) = mcp {
        error.message = mcp.redact_text(&error.message);
    }
    error
}

fn redact_handshake_error(
    error: WireError,
    stderr_lines: &[String],
    mcp: Option<&McpLaunchConfig>,
) -> WireError {
    if stderr_lines.is_empty() {
        return redact_mcp_error(error, mcp);
    }
    let message = format!(
        "{} Agent stderr: {}",
        error.message,
        stderr_lines.join(" | ")
    );
    let message = mcp
        .map(|config| config.redact_text(&message))
        .unwrap_or(message);
    WireError::new(error.code, message)
}

fn is_user_message_chunk(value: &serde_json::Value, session_id: &str) -> bool {
    let session_matches = match value.pointer("/params/sessionId") {
        Some(value) => value.as_str() == Some(session_id),
        None => {
            // This daemon starts one ACP child per session, and ACP
            // notifications may omit sessionId; an absent field is therefore
            // treated as belonging to this reader.
            true
        }
    };
    value.get("method").and_then(serde_json::Value::as_str) == Some("session/update")
        && value
            .pointer("/params/update/sessionUpdate")
            .and_then(serde_json::Value::as_str)
            == Some("user_message_chunk")
        && session_matches
}

struct AcpWriter {
    transport: Arc<AcpTransport>,
    pending: Vec<u8>,
}

impl Write for AcpWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let prompt = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        self.transport
            .send_prompt(vec![serde_json::json!({ "type": "text", "text": prompt })])?;
        Ok(())
    }
}

struct AcpKiller {
    process: Arc<Mutex<Child>>,
    transport: Arc<AcpTransport>,
    permission_broker: Arc<PermissionBroker>,
    cancelled: Arc<AtomicBool>,
}

impl SessionKiller for AcpKiller {
    /// Soft interrupt: ask the agent to cancel the current turn and release
    /// pending permission prompts. The process, the turn watch, and the
    /// kill guard stay untouched so later turns keep working.
    fn interrupt(&mut self) {
        // A kill already sent its own cancel and is tearing the peer down;
        // a late interrupt would only re-cancel a closing transport.
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        self.transport.cancel();
        self.permission_broker.cancel_pending();
    }

    fn kill(&mut self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            let watchdog = Arc::clone(&self.process);
            let _ = std::thread::Builder::new()
                .name("acp-kill-watchdog".to_string())
                .spawn(move || {
                    std::thread::sleep(Duration::from_millis(150));
                    if let Ok(mut process) = watchdog.lock() {
                        let _ = process.kill();
                    }
                });
            let started = Instant::now();
            self.permission_broker.close();
            self.transport.turn.shutdown();
            self.transport.cancel();
            let remaining = Duration::from_millis(25).saturating_sub(started.elapsed());
            if !remaining.is_zero() {
                std::thread::sleep(remaining);
            }
        }
        if let Ok(mut process) = self.process.lock() {
            let _ = process.kill();
        }
        self.transport.close_stdin();
        self.transport.host.shutdown();
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self {
            process: Arc::clone(&self.process),
            transport: Arc::clone(&self.transport),
            permission_broker: Arc::clone(&self.permission_broker),
            cancelled: Arc::clone(&self.cancelled),
        })
    }
}

struct AcpReader {
    buffer: Vec<u8>,
    discarding_oversized_line: bool,
    pending: Arc<Mutex<HashSet<u64>>>,
    model_switches: Arc<Mutex<HashMap<u64, PendingSwitch>>>,
    mode_switches: AcpModeResponses,
    session_id: String,
    permission_broker: Arc<PermissionBroker>,
    host: Arc<AcpHost>,
    turn: Arc<TurnWatch>,
    transport: Option<Arc<AcpTransport>>,
    deferred: Vec<serde_json::Value>,
    provider_id: Option<String>,
    handshake_manifest: Option<SessionEvent>,
    replay_count: AtomicU64,
    /// Number of non-text content blocks the view discarded. Without this a
    /// dropped image and an agent that never sent one look identical from the
    /// outside; the block is named and counted, never rendered here.
    unmodeled_content_count: AtomicU64,
}

impl AcpReader {
    #[allow(clippy::too_many_arguments)]
    fn new(
        pending: Arc<Mutex<HashSet<u64>>>,
        model_switches: Arc<Mutex<HashMap<u64, PendingSwitch>>>,
        mode_switches: AcpModeResponses,
        session_id: String,
        permission_broker: Arc<PermissionBroker>,
        host: Arc<AcpHost>,
        turn: Arc<TurnWatch>,
        transport: Option<Arc<AcpTransport>>,
        deferred: Vec<serde_json::Value>,
        provider_id: Option<String>,
        handshake_manifest: Option<SessionEvent>,
    ) -> Self {
        Self {
            buffer: Vec::new(),
            discarding_oversized_line: false,
            pending,
            model_switches,
            mode_switches,
            session_id,
            permission_broker,
            host,
            turn,
            transport,
            deferred,
            provider_id,
            handshake_manifest,
            replay_count: AtomicU64::new(0),
            unmodeled_content_count: AtomicU64::new(0),
        }
    }

    #[cfg(test)]
    fn for_test(
        pending: Arc<Mutex<HashSet<u64>>>,
        session_id: String,
        permission_broker: Arc<PermissionBroker>,
    ) -> Self {
        Self::for_test_on_host(
            pending,
            session_id,
            permission_broker,
            AcpHost::new(
                std::env::temp_dir(),
                std::env::temp_dir(),
                Arc::new(JobObject::new().expect("job")),
            ),
        )
    }

    #[cfg(test)]
    fn for_test_on_host(
        pending: Arc<Mutex<HashSet<u64>>>,
        session_id: String,
        permission_broker: Arc<PermissionBroker>,
        host: Arc<AcpHost>,
    ) -> Self {
        Self::new(
            pending,
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
            session_id,
            permission_broker,
            host,
            TurnWatch::new(),
            None,
            Vec::new(),
            None,
            None,
        )
    }

    #[cfg(test)]
    fn for_test_with_transport(
        pending: Arc<Mutex<HashSet<u64>>>,
        session_id: String,
        permission_broker: Arc<PermissionBroker>,
        host: Arc<AcpHost>,
        transport: Arc<AcpTransport>,
    ) -> Self {
        Self::new(
            pending,
            transport.model_switch_ids(),
            transport.mode_switch_ids(),
            session_id,
            permission_broker,
            host,
            Arc::clone(&transport.turn),
            Some(transport),
            Vec::new(),
            None,
            None,
        )
    }

    fn publish(&self, runtime: &SessionRuntime, event: SessionEvent) {
        self.publish_at_seq(runtime, event, None);
    }

    fn publish_at_seq(
        &self,
        runtime: &SessionRuntime,
        event: SessionEvent,
        event_seq: Option<u64>,
    ) {
        let event = if let Some(transport) = &self.transport {
            transport.override_manifest_effort(event)
        } else {
            event
        };
        let event = if matches!(&event, SessionEvent::SessionManifest { .. }) {
            let event = runtime.store_session_manifest(event);
            if let Some(transport) = &self.transport {
                transport.remember_manifest(&event);
            }
            event
        } else {
            event
        };
        let _ = runtime.publish_agent_event_with_seq(event, None, event_seq);
    }

    fn with_provider(&self, event: SessionEvent) -> SessionEvent {
        match event {
            SessionEvent::SessionManifest {
                provider_id,
                current_model_id,
                models,
                modes,
            } => SessionEvent::SessionManifest {
                provider_id: provider_id.or_else(|| self.provider_id.clone()),
                current_model_id,
                models,
                modes,
            },
            other => other,
        }
    }

    fn remove_model_switch(&self, id: u64) {
        match self.model_switches.lock() {
            Ok(mut switches) => {
                switches.remove(&id);
            }
            Err(poisoned) => {
                poisoned.into_inner().remove(&id);
            }
        }
    }
}

fn observe_mcp_status(value: &serde_json::Value, runtime: &SessionRuntime) {
    if !is_mcp_status(value) {
        return;
    }
    let status = value
        .pointer("/params/status")
        .and_then(serde_json::Value::as_str);
    let reason = value
        .pointer("/params/reason")
        .and_then(serde_json::Value::as_str);
    if status == Some("ready") && reason == Some("initialized") {
        // This is a provider hint only. The broker marks readiness after it
        // has authenticated and served this session's tools/list request.
        return;
    }
    if matches!(status, Some("failed") | Some("error")) {
        runtime.fail_mcp("The ACP provider reported that the MCP broker failed.");
    }
}

fn is_mcp_status(value: &serde_json::Value) -> bool {
    value.get("method").and_then(serde_json::Value::as_str) == Some("_x.ai/mcp/server_status")
        && value
            .pointer("/params/name")
            .and_then(serde_json::Value::as_str)
            == Some(crate::mcp_broker::MCP_SERVER_NAME)
}

impl ReaderDispatch for AcpReader {
    fn feed(&mut self, bytes: &[u8], runtime: &Arc<SessionRuntime>) -> Result<(), String> {
        self.turn.bind_runtime(runtime);
        self.host
            .bind_permission_gate(&self.permission_broker, runtime);
        if let Some(manifest) = self.handshake_manifest.take() {
            self.publish(runtime, manifest);
        }
        if !self.deferred.is_empty() {
            let deferred = std::mem::take(&mut self.deferred);
            for value in deferred {
                self.dispatch_value(&value, runtime);
            }
        }
        // `reader_loop` supplies arbitrary chunks. Buffering here means a
        // split UTF-8/JSON line is never parsed as two messages.
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
                if self.buffer.len() > MAX_ACP_PERMISSION_LINE_BYTES {
                    self.buffer.clear();
                    self.discarding_oversized_line = true;
                    self.publish(
                        runtime,
                        SessionEvent::AgentError {
                            message: format!(
                                "ACP input line exceeded {MAX_ACP_PERMISSION_LINE_BYTES} bytes and was discarded."
                            ),
                        },
                    );
                }
                break;
            };
            let line: Vec<u8> = self.buffer.drain(..=newline).collect();
            if line.len() > MAX_ACP_PERMISSION_LINE_BYTES {
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!(
                            "ACP input line exceeded {MAX_ACP_PERMISSION_LINE_BYTES} bytes and was discarded."
                        ),
                    },
                );
                continue;
            }
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let line = String::from_utf8_lossy(line);
            self.dispatch_line(&line, runtime);
        }
        Ok(())
    }

    fn finish(&mut self, runtime: &Arc<SessionRuntime>) {
        self.permission_broker.close();
        self.turn.shutdown();
        self.host.shutdown();
        self.transport = None;
        let replay_count = self.replay_count.load(Ordering::Relaxed);
        if replay_count > 0 {
            eprintln!(
                "session {} dropped {} ACP replay notifications during resume",
                self.session_id, replay_count
            );
        }
        let unmodeled_content = self.unmodeled_content_count.load(Ordering::Relaxed);
        if unmodeled_content > 0 {
            eprintln!(
                "session {} discarded {} non-text ACP content block(s) the view does not model",
                self.session_id, unmodeled_content
            );
        }
        if !self.buffer.is_empty() {
            eprintln!("skipping unterminated ACP output line");
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: "ACP agent ended with an unterminated output line.".to_string(),
                },
            );
        }
    }
}

fn bounded_permission_text(
    value: Option<&serde_json::Value>,
    field: &str,
    required: bool,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return if required {
            Err(format!("ACP permission request has no {field}"))
        } else {
            Ok(None)
        };
    };
    let Some(text) = value.as_str() else {
        return Err(format!("ACP permission request {field} must be a string"));
    };
    if text.is_empty() {
        return Err(format!("ACP permission request has an empty {field}"));
    }
    if text.len() > MAX_ACP_PERMISSION_FIELD_BYTES {
        return Err(format!(
            "ACP permission request {field} exceeds {MAX_ACP_PERMISSION_FIELD_BYTES} bytes"
        ));
    }
    Ok(Some(text.to_string()))
}

#[cfg(test)]
fn complete_lines(buffer: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
        lines.push(buffer.drain(..=newline).collect());
    }
    lines
}

impl AcpReader {
    fn dispatch_line(&self, line: &str, runtime: &Arc<SessionRuntime>) {
        let value = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("skipping malformed ACP output line: {error}");
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!("Malformed ACP output was skipped: {error}"),
                    },
                );
                return;
            }
        };
        self.dispatch_value(&value, runtime);
    }

    fn dispatch_value(&self, value: &serde_json::Value, runtime: &Arc<SessionRuntime>) {
        let value = runtime.redact_mcp_value(value);
        if is_mcp_status(&value) {
            observe_mcp_status(&value, runtime);
            return;
        }
        if matches!(
            classify_line(&value),
            Some(AcpLineKind::Notification { .. })
        ) && value
            .get("_meta")
            .and_then(|meta| meta.get("isReplay"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            self.replay_count.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.turn.note_activity();
        if is_user_message_chunk(&value, &self.session_id) {
            // The daemon records the outbound prompt before writing. Never
            // allocate a sequence or journal grok's redundant
            // echo: burning a sequence without a row would look like a
            // durable journal hole during live replay. `acp_view` continues
            // to map historical echo envelopes for backward compatibility.
            return;
        }
        let event_seq = runtime.journal_agent_envelope(&value);
        match classify_line(&value) {
            Some(AcpLineKind::Request { method }) => {
                if method == "session/request_permission" {
                    self.dispatch_permission(&value, runtime, event_seq);
                    return;
                }
                self.dispatch_client_request(&method, &value, runtime);
            }
            Some(AcpLineKind::Response) => {
                if let Some(id) = value.get("id").and_then(serde_json::Value::as_u64) {
                    self.dispatch_response(id, &value, runtime, event_seq);
                }
            }
            Some(AcpLineKind::Notification { .. }) => {
                if let Some(mode_id) = current_mode_id_from_update(&value, &self.session_id) {
                    self.dispatch_current_mode_update(&mode_id, runtime, event_seq);
                    return;
                }
                if value.get("method").and_then(serde_json::Value::as_str)
                    == Some("_x.ai/sessions/changed")
                {
                    self.dispatch_sessions_changed(&value, runtime, event_seq);
                    return;
                }
                if let Some(view) =
                    view_from_envelope_in(&value, &self.session_id, Some(self.host.cwd()))
                {
                    let view = self.with_provider(view);
                    if value.get("method").and_then(serde_json::Value::as_str)
                        == Some("_x.ai/models/update")
                    {
                        if let Some(transport) = &self.transport {
                            let shape = add_vendor_surface(transport.model_switch_shape(), &view);
                            transport.set_model_switch_shape(shape);
                        }
                    }
                    self.publish_at_seq(runtime, view, event_seq);
                } else if let Some(content_type) = unmodeled_content_kind(&value) {
                    // A text-bearing chunk whose content block is not text: the
                    // view returned `None`. Count and name it so a discarded
                    // image is not indistinguishable from an absent one.
                    let previous = self.unmodeled_content_count.fetch_add(1, Ordering::Relaxed);
                    if previous == 0 {
                        eprintln!(
                            "session {} is discarding a non-text '{}' content block the ACP view does not model",
                            self.session_id, content_type
                        );
                    }
                }
            }
            None => {}
        }
    }

    fn dispatch_current_mode_update(
        &self,
        mode_id: &str,
        runtime: &SessionRuntime,
        event_seq: Option<u64>,
    ) {
        let Some(transport) = &self.transport else {
            return;
        };
        let Some(SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes: Some(mut modes),
        }) = transport
            .last_manifest()
            .or_else(|| runtime.session_manifest())
        else {
            return;
        };
        modes.current_mode_id = mode_id.to_string();
        self.publish_at_seq(
            runtime,
            self.with_provider(SessionEvent::SessionManifest {
                provider_id,
                current_model_id,
                models,
                modes: Some(modes),
            }),
            event_seq,
        );
    }

    fn dispatch_sessions_changed(
        &self,
        value: &serde_json::Value,
        runtime: &SessionRuntime,
        event_seq: Option<u64>,
    ) {
        let Some(upserted) = value
            .pointer("/params/upserted")
            .and_then(serde_json::Value::as_array)
        else {
            return;
        };
        let Some(update) = upserted.iter().find(|entry| {
            entry.get("sessionId").and_then(serde_json::Value::as_str)
                == Some(self.session_id.as_str())
        }) else {
            return;
        };
        let model_id = update
            .get("modelId")
            .and_then(serde_json::Value::as_str)
            .filter(|model_id| !model_id.is_empty())
            .map(str::to_string);
        let effort = update
            .get("reasoningEffort")
            .and_then(serde_json::Value::as_str)
            .filter(|effort| !effort.is_empty())
            .map(str::to_string);
        if model_id.is_none() && effort.is_none() {
            return;
        }
        let Some(transport) = &self.transport else {
            return;
        };
        transport.update_manifest_from_sessions_changed(model_id.clone(), effort);
        let Some(SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes,
        }) = transport.last_manifest()
        else {
            return;
        };
        self.publish_at_seq(
            runtime,
            SessionEvent::SessionManifest {
                provider_id,
                current_model_id: model_id.or(current_model_id),
                models,
                modes,
            },
            event_seq,
        );
    }

    fn dispatch_client_request(
        &self,
        method: &str,
        value: &serde_json::Value,
        runtime: &Arc<SessionRuntime>,
    ) {
        let Some(id) = value.get("id").cloned() else {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: format!("ACP client request {method} had no id."),
                },
            );
            return;
        };
        let params = value
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let Some(transport) = &self.transport else {
            return;
        };
        self.turn.begin_client_work();
        let turn = Arc::clone(&self.turn);
        let transport = Arc::clone(transport);
        let respond: RpcRespond = Arc::new(move |response_id, result| {
            turn.end_client_work();
            let _ = transport.send_result(response_id, result);
        });
        self.host.dispatch(method, id, params, respond);
    }

    fn send_switch_request(
        &self,
        request: SwitchRequest,
        alternate: Option<AlternateSwitch>,
        requested_model_id: Option<String>,
        requested_effort: Option<String>,
        followup: Option<FollowupSwitch>,
    ) -> io::Result<()> {
        let transport = self
            .transport
            .as_ref()
            .ok_or_else(|| io::Error::other("ACP transport is no longer live"))?;
        match request {
            SwitchRequest::Vendor { model_id, effort } => transport
                .request_set_model(model_id, effort, alternate, followup)
                .map(|_| ()),
            SwitchRequest::Config {
                config_id,
                value,
                control,
            } => transport
                .request_set_config_option(
                    &config_id,
                    &value,
                    control,
                    requested_model_id,
                    requested_effort,
                    alternate,
                    followup,
                )
                .map(|_| ()),
        }
    }

    fn retry_switch_or_publish_error(
        &self,
        runtime: &SessionRuntime,
        error: &serde_json::Value,
        pending: PendingSwitch,
    ) {
        let original = acp_request_error_message(error);
        let (alternate, requested_model_id, requested_effort, followup) = match pending {
            PendingSwitch::SetModel {
                model_id,
                effort,
                alternate,
                followup,
            } => (alternate, Some(model_id), effort, followup),
            PendingSwitch::SetConfigOption {
                requested_model_id,
                requested_effort,
                alternate,
                followup,
                ..
            } => (alternate, requested_model_id, requested_effort, followup),
        };
        match alternate {
            Some(AlternateSwitch::Blocked(reason)) => self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: format!("{original}; {reason}; the original error is unchanged"),
                },
            ),
            Some(AlternateSwitch::Request(request)) => {
                // Keep the follow-up even when the vendor request carries an
                // effort hint: a legacy peer may accept the model while
                // ignoring that hint, so the declared effort surface must
                // still get its own request.
                if let Err(retry_error) = self.send_switch_request(
                    request,
                    None,
                    requested_model_id,
                    requested_effort,
                    followup,
                ) {
                    self.publish(
                        runtime,
                        SessionEvent::AgentError {
                            message: format!(
                                "{original}; the alternate switch surface also failed to send: \
                                 {retry_error}"
                            ),
                        },
                    );
                }
            }
            None => self.publish(runtime, SessionEvent::AgentError { message: original }),
        }
    }

    fn complete_vendor_switch(
        &self,
        runtime: &SessionRuntime,
        model_id: String,
        effort: Option<String>,
        followup: Option<FollowupSwitch>,
        event_seq: Option<u64>,
    ) {
        let Some(transport) = &self.transport else {
            return;
        };
        // A successful JSON-RPC response is success even when the legacy
        // peer omits `_meta.model.Ok`; the requested values are the only
        // usable acknowledgement in that case.
        transport.update_current_model_id(Some(model_id.clone()));
        if effort.is_some() {
            transport.update_current_effort(effort.clone());
        }
        let manifest = match transport.last_manifest() {
            Some(SessionEvent::SessionManifest {
                provider_id,
                models,
                modes,
                ..
            }) => {
                let models = models
                    .into_iter()
                    .map(|mut model| {
                        if model.model_id == model_id {
                            if let Some(effort) = &effort {
                                model.current_effort = Some(effort.clone());
                            }
                        }
                        model
                    })
                    .collect();
                SessionEvent::SessionManifest {
                    provider_id,
                    current_model_id: Some(model_id.clone()),
                    models,
                    modes,
                }
            }
            _ => SessionEvent::SessionManifest {
                provider_id: self.provider_id.clone(),
                current_model_id: Some(model_id.clone()),
                models: Vec::new(),
                modes: None,
            },
        };
        self.publish_at_seq(runtime, manifest, event_seq);
        self.send_followup(runtime, followup);
    }

    fn send_followup(&self, runtime: &SessionRuntime, followup: Option<FollowupSwitch>) {
        let Some(followup) = followup else {
            return;
        };
        if let Err(error) = self.send_switch_request(
            followup.request,
            followup.alternate,
            followup.requested_model_id,
            followup.requested_effort,
            None,
        ) {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: format!(
                        "The first switch succeeded, but its follow-up failed: {error}"
                    ),
                },
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn complete_config_switch(
        &self,
        runtime: &SessionRuntime,
        response: &serde_json::Value,
        config_id: String,
        control: ConfigControl,
        requested_model_id: Option<String>,
        requested_effort: Option<String>,
        followup: Option<FollowupSwitch>,
        event_seq: Option<u64>,
    ) {
        let Some(transport) = &self.transport else {
            return;
        };
        let previous = transport.last_manifest();
        let (provider_id, modes) = match previous.as_ref() {
            Some(SessionEvent::SessionManifest {
                provider_id, modes, ..
            }) => (provider_id.clone(), modes.clone()),
            _ => (self.provider_id.clone(), None),
        };
        let known_ids = transport.model_switch_shape().and_then(|shape| {
            if let (Some(model), effort) = (shape.model.config, shape.effort.config) {
                Some((model.id, effort.map(|option| option.id)))
            } else {
                None
            }
        });
        let known_refs = known_ids
            .as_ref()
            .map(|(model, effort)| (model.as_str(), effort.as_deref()));
        let catalog = response.get("result").and_then(|result| {
            catalog_from_config_options(result, provider_id.clone(), modes.clone(), known_refs)
        });

        let response_model_id = catalog
            .as_ref()
            .and_then(|catalog| match &catalog.manifest {
                SessionEvent::SessionManifest {
                    current_model_id, ..
                } => current_model_id.clone(),
                _ => None,
            });
        let manifest = catalog
            .as_ref()
            .map(|catalog| catalog.manifest.clone())
            .or(previous)
            .unwrap_or_else(|| SessionEvent::SessionManifest {
                provider_id: provider_id.clone(),
                current_model_id: None,
                models: Vec::new(),
                modes: modes.clone(),
            });
        let SessionEvent::SessionManifest {
            provider_id: manifest_provider,
            mut current_model_id,
            mut models,
            modes: manifest_modes,
        } = manifest
        else {
            return;
        };
        let reported_effort = catalog.as_ref().and_then(|_| {
            current_model_id.as_deref().and_then(|model_id| {
                models
                    .iter()
                    .find(|model| model.model_id == model_id)
                    .and_then(|model| model.current_effort.clone())
            })
        });
        if matches!(control, ConfigControl::Model) && response_model_id.is_none() {
            current_model_id = requested_model_id.clone().or(current_model_id);
        }
        let effective_effort = reported_effort.or_else(|| match control {
            ConfigControl::Effort => requested_effort.clone(),
            ConfigControl::Model => None,
        });
        if let Some(current_model_id) = current_model_id.as_deref() {
            for model in &mut models {
                if model.model_id == current_model_id {
                    if let Some(effort) = &effective_effort {
                        model.current_effort = Some(effort.clone());
                    }
                }
            }
        }
        transport.update_current_model_id(current_model_id.clone());
        if effective_effort.is_some() {
            transport.update_current_effort(effective_effort.clone());
        }
        if let Some(catalog) = catalog {
            if let Some(mut shape) = transport.model_switch_shape() {
                shape.model.config = Some(ConfigOptionSurface {
                    id: catalog.model_option_id,
                    values: catalog.model_values,
                });
                if let Some(effort_id) = catalog.effort_option_id {
                    shape.effort.config = Some(ConfigOptionSurface {
                        id: effort_id,
                        values: catalog.effort_values,
                    });
                }
                transport.set_model_switch_shape(Some(shape));
            }
        }
        self.publish_at_seq(
            runtime,
            SessionEvent::SessionManifest {
                provider_id: manifest_provider,
                current_model_id,
                models,
                modes: manifest_modes,
            },
            event_seq,
        );
        self.send_followup(runtime, followup);
        let _ = config_id;
    }

    fn dispatch_response(
        &self,
        id: u64,
        value: &serde_json::Value,
        runtime: &SessionRuntime,
        event_seq: Option<u64>,
    ) {
        let response_was_pending = match self.pending.lock() {
            Ok(mut pending) => pending.remove(&id),
            Err(poisoned) => {
                poisoned.into_inner().remove(&id);
                self.remove_model_switch(id);
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!(
                            "ACP response tracking lock was poisoned while handling response {id}; \
                             the switch outcome is unknown"
                        ),
                    },
                );
                return;
            }
        };
        if !response_was_pending {
            eprintln!("skipping ACP response with unknown id {id}");
            return;
        }
        let mode_sender = self
            .mode_switches
            .lock()
            .map(|mut switches| switches.remove(&id))
            .unwrap_or(None);
        if let Some(sender) = mode_sender {
            let result = value
                .get("error")
                .map_or_else(|| Ok(()), |error| Err(acp_request_error_message(error)));
            let _ = sender.send(result);
            return;
        }
        let pending_switch = match self.model_switches.lock() {
            Ok(mut switches) => switches.remove(&id),
            Err(poisoned) => {
                poisoned.into_inner().remove(&id);
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!(
                            "ACP model-switch tracking lock was poisoned while handling response {id}; \
                             the switch outcome is unknown"
                        ),
                    },
                );
                return;
            }
        };
        if let Some(pending) = pending_switch {
            if let Some(error) = value.get("error") {
                self.retry_switch_or_publish_error(runtime, error, pending);
                return;
            }
            match pending {
                PendingSwitch::SetModel {
                    model_id,
                    effort,
                    followup,
                    ..
                } => self.complete_vendor_switch(runtime, model_id, effort, followup, event_seq),
                PendingSwitch::SetConfigOption {
                    config_id,
                    value: _config_value,
                    control,
                    requested_model_id,
                    requested_effort,
                    followup,
                    ..
                } => self.complete_config_switch(
                    runtime,
                    value,
                    config_id,
                    control,
                    requested_model_id,
                    requested_effort,
                    followup,
                    event_seq,
                ),
            }
            return;
        }
        if !self.turn.finish_prompt(id) {
            return;
        }
        if let Some(error) = value.get("error") {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: format!(
                        "ACP request {id} failed: {}",
                        acp_request_error_message(error)
                    ),
                },
            );
            return;
        }
        if let Some(view) = view_from_envelope_in(value, &self.session_id, Some(self.host.cwd())) {
            self.publish_at_seq(runtime, view, event_seq);
        }
    }

    fn cancel_permission_request(&self, id: u64, runtime: &SessionRuntime, reason: String) {
        let _ = self.permission_broker.send(
            id,
            serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
        );
        self.publish(runtime, SessionEvent::AgentError { message: reason });
    }

    fn dispatch_permission(
        &self,
        value: &serde_json::Value,
        runtime: &Arc<SessionRuntime>,
        event_seq: Option<u64>,
    ) {
        let Some(id) = value.get("id").and_then(serde_json::Value::as_u64) else {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: "ACP permission request had no numeric id.".to_string(),
                },
            );
            return;
        };
        let Some(params) = value.get("params") else {
            self.cancel_permission_request(
                id,
                runtime,
                "ACP permission request had no params and was cancelled.".to_string(),
            );
            return;
        };
        if params.get("sessionId").and_then(serde_json::Value::as_str)
            != Some(self.session_id.as_str())
        {
            self.cancel_permission_request(
                id,
                runtime,
                "ACP permission request targeted another session and was cancelled.".to_string(),
            );
            return;
        }
        let tool_call = params
            .get("toolCall")
            .or_else(|| {
                params
                    .get("subject")
                    .and_then(|subject| subject.get("toolCall"))
            })
            .and_then(|tool_call| tool_call.get("toolCall").or(Some(tool_call)));
        let tool_call_id_value = tool_call
            .and_then(|tool_call| tool_call.get("toolCallId"))
            .or_else(|| params.get("toolCallId"));
        let options_value = params.get("options");
        let parsed = (|| -> Result<SessionEvent, String> {
            let tool_call_id = bounded_permission_text(tool_call_id_value, "tool_call_id", true)?
                .expect("required permission field has a value");
            let raw_options = options_value
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| "ACP permission request had no options array".to_string())?;
            if raw_options.is_empty() {
                return Err("ACP permission request had no options".to_string());
            }
            if raw_options.len() > MAX_ACP_PERMISSION_OPTIONS {
                return Err(format!(
                    "ACP permission request has more than the maximum of {MAX_ACP_PERMISSION_OPTIONS} options"
                ));
            }
            let options = raw_options
                .iter()
                .map(|option| {
                    Ok(PermissionOption {
                        option_id: bounded_permission_text(
                            option.get("optionId"),
                            "option_id",
                            true,
                        )?
                        .expect("required permission field has a value"),
                        name: bounded_permission_text(option.get("name"), "option name", true)?
                            .expect("required permission field has a value"),
                        kind: bounded_permission_text(option.get("kind"), "option kind", true)?
                            .expect("required permission field has a value"),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let title = params
                .get("title")
                .or_else(|| tool_call.and_then(|call| call.get("title")))
                .map_or_else(
                    || Ok("Permission requested".to_string()),
                    |value| {
                        bounded_permission_text(Some(value), "title", true)
                            .map(|value| value.expect("required permission field has a value"))
                    },
                )?;
            let description =
                bounded_permission_text(params.get("description"), "description", false)?;
            let command = bounded_permission_text(
                params.get("command").or_else(|| {
                    params
                        .get("subject")
                        .and_then(|subject| subject.get("command"))
                }),
                "command",
                false,
            )?;
            let cwd = bounded_permission_text(
                params
                    .get("cwd")
                    .or_else(|| params.get("subject").and_then(|subject| subject.get("cwd"))),
                "cwd",
                false,
            )?;
            Ok(SessionEvent::PermissionRequest {
                tool_call_id,
                title,
                description,
                command,
                args: None,
                cwd,
                env: None,
                options,
            })
        })();
        let event = match parsed {
            Ok(event) => event,
            Err(reason) => {
                self.cancel_permission_request(
                    id,
                    runtime,
                    format!("ACP permission request was rejected: {reason}."),
                );
                return;
            }
        };
        let tool_call_id = match &event {
            SessionEvent::PermissionRequest { tool_call_id, .. } => tool_call_id.clone(),
            _ => unreachable!("permission parser returned a different event"),
        };
        let delivery = runtime.permission_delivery_enabled();
        if !self.turn.prompt_is_live() {
            self.cancel_permission_request(
                id,
                runtime,
                "ACP permission request arrived after the turn ended and was cancelled."
                    .to_string(),
            );
            return;
        }
        let pending = match self.permission_broker.register(id, event.clone(), runtime) {
            Ok(pending) => pending,
            Err(error) => {
                self.cancel_permission_request(
                    id,
                    runtime,
                    format!("Could not queue ACP permission request: {error}"),
                );
                return;
            }
        };
        if !self.turn.prompt_is_live() {
            let _ = self
                .permission_broker
                .cancel(&tool_call_id, &pending, "cancelled");
            return;
        }
        match self.permission_broker.auto_answer(&tool_call_id, runtime) {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!("Could not auto-answer ACP permission request: {error}"),
                    },
                );
                return;
            }
        }
        if delivery == Some(false) {
            let _ =
                self.permission_broker
                    .cancel(&tool_call_id, &pending, "capability_not_supported");
            return;
        }
        let _ = runtime.publish_agent_event_with_seq(event, None, event_seq);
    }
}

struct AcpStderr {
    state: Arc<Mutex<AcpStderrState>>,
    handle: Option<JoinHandle<()>>,
}

struct AcpStderrState {
    runtime: Option<Arc<SessionRuntime>>,
    pending: VecDeque<String>,
}

impl AcpStderr {
    fn start(stderr: ChildStderr) -> io::Result<Self> {
        let state = Arc::new(Mutex::new(AcpStderrState {
            runtime: None,
            pending: VecDeque::new(),
        }));
        let thread_state = Arc::clone(&state);
        let handle = std::thread::Builder::new()
            .name("session-acp-stderr".to_string())
            .spawn(move || {
                let mut reader = BufReader::new(stderr);
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) => return,
                        Ok(_) => {
                            let line = line
                                .trim_end_matches('\n')
                                .trim_end_matches('\r')
                                .to_string();
                            let runtime = match thread_state.lock() {
                                Ok(mut state) => {
                                    if let Some(runtime) = &state.runtime {
                                        Some(Arc::clone(runtime))
                                    } else {
                                        if state.pending.len() < 256 {
                                            state.pending.push_back(line.clone());
                                        } else {
                                            eprintln!(
                                                "dropping ACP stderr while handshake is pending"
                                            );
                                        }
                                        None
                                    }
                                }
                                Err(_) => return,
                            };
                            if let Some(runtime) = runtime {
                                publish_stderr_line(&runtime, line);
                            }
                        }
                        Err(error) => {
                            let runtime = thread_state
                                .lock()
                                .ok()
                                .and_then(|state| state.runtime.clone());
                            if let Some(runtime) = runtime {
                                let _ = runtime.publish_agent_event(
                                    SessionEvent::AgentError {
                                        message: format!("Could not read ACP stderr: {error}"),
                                    },
                                    None,
                                );
                            } else {
                                eprintln!("could not read ACP stderr: {error}");
                            }
                            return;
                        }
                    }
                }
            })?;
        Ok(Self {
            state,
            handle: Some(handle),
        })
    }

    fn discard_and_join(&mut self) -> Vec<String> {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        if let Ok(mut state) = self.state.lock() {
            return state.pending.drain(..).collect();
        }
        Vec::new()
    }
}

impl StderrSource for AcpStderr {
    fn spawn(mut self: Box<Self>, runtime: Arc<SessionRuntime>) -> io::Result<JoinHandle<()>> {
        let pending = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| io::Error::other("ACP stderr lock poisoned"))?;
            state.runtime = Some(Arc::clone(&runtime));
            std::mem::take(&mut state.pending)
        };
        for line in pending {
            publish_stderr_line(&runtime, line);
        }
        self.handle
            .take()
            .ok_or_else(|| io::Error::other("ACP stderr drain was already consumed"))
    }
}

fn publish_stderr_line(runtime: &SessionRuntime, line: String) {
    let _ = runtime.publish_agent_event(
        SessionEvent::AgentStderr {
            data: runtime.redact_mcp_text(&line),
        },
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::super::permission_broker::{
        permission, permission_path, test_broker, PermissionBroker, MAX_ACP_PERMISSION_FIELD_BYTES,
    };
    use super::{
        acp_request_error_message, complete_lines, is_mcp_status, observe_mcp_status,
        redact_handshake_error, AcpReader, PendingSwitch, MAX_ACP_PERMISSION_LINE_BYTES,
    };
    use crate::journal::Journal;
    use crate::session::{ConnHandle, ReaderDispatch, SessionKiller, SessionRuntime};
    use devboule_protocol::{
        ErrorCode, PermissionOutcome, SessionEvent, SessionKind, SessionModel, WireError,
    };
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn mcp_status_is_parsed_as_a_hint_and_failure_is_reported() {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.require_mcp();
        let ready = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "_x.ai/mcp/server_status",
            "params": {
                "name": "devboule",
                "status": "ready",
                "reason": "initialized"
            }
        });
        assert!(is_mcp_status(&ready));
        observe_mcp_status(&ready, &runtime);
        assert!(runtime
            .wait_for_mcp_ready(Duration::from_millis(1))
            .is_err());

        let failed = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "_x.ai/mcp/server_status",
            "params": {
                "name": "devboule",
                "status": "failed"
            }
        });
        observe_mcp_status(&failed, &runtime);
        let error = runtime
            .wait_for_mcp_ready(Duration::from_secs(1))
            .expect_err("provider failure must wake the gate");
        assert!(error.message.contains("ACP provider reported"));
    }

    #[test]
    fn mcp_bearer_is_redacted_from_stderr_before_delivery() {
        let (broker, _) = test_broker();
        let (runtime, conn) = attached_runtime("stderr-redaction", broker);
        runtime.set_mcp_bearer("opaque-bearer".to_string());
        runtime.set_mcp_url("http://127.0.0.1:4567/mcp".to_string());
        super::publish_stderr_line(
            &runtime,
            "provider echoed Bearer opaque-bearer at http://127.0.0.1:4567/mcp".to_string(),
        );
        let event = conn
            .pull_events()
            .into_iter()
            .find_map(|event| match event.envelope.event {
                SessionEvent::AgentStderr { data } => Some(data),
                _ => None,
            })
            .expect("stderr event");
        assert_eq!(event, "provider echoed Bearer [redacted] at [redacted]");
        let journal_value = runtime.redact_mcp_value(&serde_json::json!({
            "echo": "Bearer opaque-bearer at http://127.0.0.1:4567/mcp"
        }));
        let journal_text = serde_json::to_string(&journal_value).expect("redacted JSON");
        assert!(!journal_text.contains("opaque-bearer"));
        assert!(!journal_text.contains("4567"));
    }

    #[test]
    fn spawn_handshake_errors_redact_broker_details_with_and_without_stderr() {
        let config = crate::mcp_broker::McpLaunchConfig::for_test(
            "http://127.0.0.1:4567/mcp",
            "opaque-bearer",
        );
        let without_stderr = redact_handshake_error(
            WireError::new(
                ErrorCode::Io,
                "ACP request failed: Bearer opaque-bearer at http://127.0.0.1:4567/mcp",
            ),
            &[],
            Some(&config),
        );
        assert!(!without_stderr.message.contains("opaque-bearer"));
        assert!(!without_stderr.message.contains("4567"));

        let with_stderr = redact_handshake_error(
            WireError::new(ErrorCode::Io, "ACP request failed: handshake rejected"),
            &["provider echoed Bearer opaque-bearer at http://127.0.0.1:4567/mcp".to_string()],
            Some(&config),
        );
        assert!(with_stderr.message.contains("Agent stderr"));
        assert!(!with_stderr.message.contains("opaque-bearer"));
        assert!(!with_stderr.message.contains("4567"));
    }

    #[test]
    fn request_error_without_a_message_never_serializes_the_object() {
        let error = serde_json::json!({
            "code": -32000,
            "data": {"token": "sk-LEAK"}
        });
        let text = acp_request_error_message(&error);
        assert!(
            !text.contains("sk-LEAK"),
            "structured error data must not reach a user-facing banner: {text}"
        );
        assert!(text.contains("(-32000)"), "code must stay visible: {text}");
    }

    #[test]
    fn poisoned_pending_response_tracking_publishes_an_agent_error() {
        let pending = Arc::new(Mutex::new(HashSet::from([7_u64])));
        let poisoned = Arc::clone(&pending);
        let panic = thread::spawn(move || {
            let _guard = poisoned.lock().expect("pending lock");
            panic!("poison pending lock");
        })
        .join();
        assert!(panic.is_err());

        let (broker, _) = test_broker();
        let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
        let reader = AcpReader::for_test(pending, "stub-session".to_string(), broker);
        reader.dispatch_line(r#"{"jsonrpc":"2.0","id":7,"result":{}}"#, &runtime);

        let events = conn.pull_events();
        assert!(events.iter().any(|event| {
            matches!(
                &event.envelope.event,
                SessionEvent::AgentError { message }
                    if message.contains("response tracking lock was poisoned")
                        && message.contains("switch outcome is unknown")
            )
        }));
    }

    #[cfg(windows)]
    #[test]
    fn vendor_switch_without_a_manifest_still_publishes_the_new_model() {
        use super::{AcpHost, AcpTransport};
        use crate::process_tree::JobObject;
        use std::process::{Command, Stdio};

        let mut child = Command::new("cmd.exe")
            .args(["/c", "exit"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cmd");
        let stdin = child.stdin.take().expect("stdin");
        let cwd = std::env::temp_dir();
        let host = AcpHost::new(cwd.clone(), cwd, Arc::new(JobObject::new().expect("job")));
        let transport = Arc::new(AcpTransport::new(stdin, Arc::clone(&host)));
        transport.set_session_id("stub-session".to_string());
        let (broker, _) = test_broker();
        let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
        let reader = AcpReader::for_test_with_transport(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            broker,
            host,
            transport,
        );

        reader.complete_vendor_switch(&runtime, "fallback-model".to_string(), None, None, None);

        let events = conn.pull_events();
        assert!(events.iter().any(|event| {
            matches!(
                &event.envelope.event,
                SessionEvent::SessionManifest {
                    current_model_id: Some(model_id),
                    ..
                } if model_id == "fallback-model"
            )
        }));
        let _ = child.wait();
    }

    #[cfg(windows)]
    #[test]
    fn negotiated_prompt_capabilities_are_kept_on_the_session_transport() {
        use super::{AcpHost, AcpTransport};
        use crate::acp_view::{PromptCapabilities, PromptCapabilityState};
        use crate::process_tree::JobObject;
        use std::process::{Command, Stdio};

        let mut child = Command::new("cmd.exe")
            .args(["/c", "exit"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cmd");
        let stdin = child.stdin.take().expect("stdin");
        let cwd = std::env::temp_dir();
        let host = AcpHost::new(cwd.clone(), cwd, Arc::new(JobObject::new().expect("job")));
        let transport = AcpTransport::new(stdin, host);

        // A session that never declared anything stays absent, not `false`.
        assert_eq!(
            transport.prompt_capabilities(),
            PromptCapabilities::default()
        );

        transport.set_prompt_capabilities(PromptCapabilities {
            image: PromptCapabilityState::Supported,
            audio: PromptCapabilityState::Unsupported,
            embedded_context: PromptCapabilityState::Absent,
        });
        let stored = transport.prompt_capabilities();
        assert_eq!(stored.image, PromptCapabilityState::Supported);
        assert_eq!(stored.audio, PromptCapabilityState::Unsupported);
        assert_eq!(stored.embedded_context, PromptCapabilityState::Absent);
        let _ = child.wait();
    }

    #[cfg(windows)]
    #[test]
    fn poisoned_manifest_lock_preserves_the_prior_model_catalog() {
        use super::{AcpHost, AcpTransport};
        use crate::process_tree::JobObject;
        use std::process::{Command, Stdio};

        let mut child = Command::new("cmd.exe")
            .args(["/c", "exit"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cmd");
        let stdin = child.stdin.take().expect("stdin");
        let cwd = std::env::temp_dir();
        let host = AcpHost::new(cwd.clone(), cwd, Arc::new(JobObject::new().expect("job")));
        let transport = Arc::new(AcpTransport::new(stdin, Arc::clone(&host)));
        transport.remember_manifest(&SessionEvent::SessionManifest {
            provider_id: Some("stub".to_string()),
            current_model_id: Some("old-model".to_string()),
            models: vec![
                SessionModel {
                    model_id: "old-model".to_string(),
                    name: "Old model".to_string(),
                    description: None,
                    context_tokens: None,
                    current_effort: None,
                    efforts: None,
                },
                SessionModel {
                    model_id: "new-model".to_string(),
                    name: "New model".to_string(),
                    description: None,
                    context_tokens: None,
                    current_effort: None,
                    efforts: None,
                },
            ],
            modes: None,
        });
        let poisoned = Arc::clone(&transport);
        let panic = thread::spawn(move || {
            let _guard = poisoned.last_manifest.lock().expect("manifest lock");
            panic!("poison manifest lock");
        })
        .join();
        assert!(panic.is_err());

        let (broker, _) = test_broker();
        let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
        let reader = AcpReader::for_test_with_transport(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            broker,
            host,
            transport,
        );
        reader.complete_vendor_switch(&runtime, "new-model".to_string(), None, None, None);

        let events = conn.pull_events();
        assert!(events.iter().any(|event| {
            matches!(
                &event.envelope.event,
                SessionEvent::SessionManifest {
                    current_model_id: Some(model_id),
                    models,
                    ..
                } if model_id == "new-model"
                    && models.len() == 2
                    && models.iter().any(|model| model.model_id == "new-model")
            )
        }));
        let _ = child.wait();
    }

    #[test]
    fn turn_error_does_not_publish_structured_error_data() {
        let (broker, _) = test_broker();
        let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
        let mut reader = AcpReader::for_test(
            Arc::new(Mutex::new(HashSet::from([9u64]))),
            "stub-session".to_string(),
            broker,
        );
        reader.turn.start_prompt(9);
        reader
            .feed(
                br#"{"jsonrpc":"2.0","id":9,"error":{"code":-32602,"message":"unknown model","data":{"secret":"do-not-publish"}}}
"#,
                &runtime,
            )
            .expect("feed");
        let message = conn
            .pull_events()
            .into_iter()
            .find_map(|event| match event.envelope.event {
                SessionEvent::AgentError { message } => Some(message),
                _ => None,
            })
            .expect("turn error event");
        assert_eq!(
            message,
            "ACP request 9 failed: ACP request failed (-32602): unknown model"
        );
        assert!(!message.contains("do-not-publish"));
    }

    #[test]
    fn skipped_user_echo_does_not_burn_a_stream_sequence() {
        let (broker, _) = test_broker();
        let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
        let reader = AcpReader::for_test(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            broker,
        );
        reader.dispatch_line(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"stub-session","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"echo"}}}}
"#,
            &runtime,
        );
        assert_eq!(
            runtime.current_agent_seq(),
            0,
            "skipped echo consumed a seq"
        );
        reader.dispatch_line(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"stub-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"reply"}}}}
"#,
            &runtime,
        );
        let _event = conn
            .pull_events()
            .into_iter()
            .find(|event| matches!(event.envelope.event, SessionEvent::AgentMessage { .. }))
            .expect("reply event");
        assert_eq!(runtime.current_agent_seq(), 1);
    }

    #[test]
    fn foreign_user_echo_is_not_silently_dropped() {
        let (broker, _) = test_broker();
        let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
        let reader = AcpReader::for_test(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            broker,
        );
        reader.dispatch_line(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"other-session","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"foreign echo"}}}}
"#,
            &runtime,
        );
        assert_eq!(runtime.current_agent_seq(), 1);
        assert!(conn.pull_events().is_empty());
    }

    #[test]
    fn non_text_content_chunk_is_counted_not_silently_dropped() {
        let (broker, _) = test_broker();
        let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
        let reader = AcpReader::for_test(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            broker,
        );
        assert_eq!(reader.unmodeled_content_count.load(Ordering::Relaxed), 0);
        reader.dispatch_line(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"stub-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"image","mimeType":"image/png","data":"AAAA"}}}}
"#,
            &runtime,
        );
        assert_eq!(
            reader.unmodeled_content_count.load(Ordering::Relaxed),
            1,
            "an image block must be counted, not discarded without trace"
        );
        assert!(
            !conn
                .pull_events()
                .iter()
                .any(|event| matches!(event.envelope.event, SessionEvent::AgentMessage { .. })),
            "an image block must not be rendered as an empty message"
        );

        // A text chunk is modeled, so it must not move the counter.
        reader.dispatch_line(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"stub-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}}}
"#,
            &runtime,
        );
        assert_eq!(reader.unmodeled_content_count.load(Ordering::Relaxed), 1);
        assert!(conn.pull_events().iter().any(|event| matches!(
            event.envelope.event,
            SessionEvent::AgentMessage { ref text, .. } if text == "hello"
        )));
    }

    #[test]
    fn model_switch_response_does_not_finish_a_live_prompt() {
        let (broker, _) = test_broker();
        let (runtime, _conn) = attached_runtime("stub-session", Arc::clone(&broker));
        let pending = Arc::new(Mutex::new(HashSet::from([42u64])));
        let reader = AcpReader::for_test(Arc::clone(&pending), "stub-session".to_string(), broker);
        reader
            .model_switches
            .lock()
            .expect("model-switch lock")
            .insert(
                42,
                PendingSwitch::SetModel {
                    model_id: "new-model".to_string(),
                    effort: None,
                    alternate: None,
                    followup: None,
                },
            );
        reader.turn.start_prompt(42);
        let mut reader = reader;
        reader
            .feed(
                br#"{"jsonrpc":"2.0","id":42,"result":{"_meta":{"model":{"Ok":"new-model"}}}}
"#,
                &runtime,
            )
            .expect("feed");
        assert!(
            reader.turn.prompt_is_live(),
            "a model-switch response must not finish a live prompt"
        );
    }

    #[test]
    fn journal_keeps_raw_envelope_and_replay_derives_the_view() {
        let path = permission_path("envelope");
        let journal = Journal::open(&path).expect("journal");
        journal
            .upsert_blocking(crate::journal::new_session_record(
                "s.envelope",
                "owner",
                None,
                devboule_protocol::SessionKind::Acp,
                "Agent",
            ))
            .expect("upsert");
        let envelope = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "01a06c70-ea2b-7882-ad27-aae8188fc243",
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "content": {"type": "text", "text": "The"}
                }
            }
        });
        journal
            .append_blocking(
                crate::journal::acp_envelope_record("s.envelope", 1, 1, &envelope).expect("record"),
            )
            .expect("append");
        let replay = journal.replay("s.envelope", 0).expect("replay");
        assert!(
            replay.events.iter().any(|event| matches!(
                event,
                SessionEvent::AgentThought { text, .. } if text == "The"
            )),
            "replay lost the derived thought: {:?}",
            replay.events
        );
        journal.shutdown();
        let stored: serde_json::Value = {
            let conn = rusqlite::Connection::open(&path).expect("inspect");
            let payload: Vec<u8> = conn
                .query_row(
                    "SELECT payload FROM events WHERE session_id = ?1 AND kind = 'acp_envelope'",
                    ["s.envelope"],
                    |row| row.get(0),
                )
                .expect("payload");
            serde_json::from_slice(&payload).expect("json")
        };
        assert_eq!(stored["method"], "session/update");
        assert_eq!(
            stored["params"]["update"]["sessionUpdate"],
            "agent_thought_chunk"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn old_journaled_user_echo_still_replays_as_a_user_message() {
        let path = permission_path("old-user-echo-replay");
        let journal = Journal::open(&path).expect("journal");
        journal
            .upsert_blocking(crate::journal::new_session_record(
                "s.old-user-echo",
                "owner",
                None,
                SessionKind::Acp,
                "Agent",
            ))
            .expect("upsert");
        let envelope = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "stub-session",
                "update": {
                    "sessionUpdate": "user_message_chunk",
                    "content": {"type": "text", "text": "old prompt"}
                }
            }
        });
        journal
            .append_blocking(
                crate::journal::acp_envelope_record("s.old-user-echo", 1, 1, &envelope)
                    .expect("record"),
            )
            .expect("append");
        let replay = journal.replay("s.old-user-echo", 0).expect("replay");
        assert!(replay.events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentUserMessage { text, .. } if text == "old prompt"
        )));
        journal.shutdown();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn initialize_declares_only_implemented_fs_and_terminal() {
        let params = super::advertised_initialize_params().expect("initialize params");
        assert_eq!(params["clientCapabilities"]["fs"]["readTextFile"], true);
        assert_eq!(params["clientCapabilities"]["fs"]["writeTextFile"], true);
        assert_eq!(params["clientCapabilities"]["terminal"], true);
        assert!(
            params["clientCapabilities"].get("elicitation").is_none()
                || params["clientCapabilities"]["elicitation"].is_null()
        );
        assert_eq!(params["clientInfo"]["name"], "devboule");
    }

    #[test]
    fn ndjson_buffers_partial_lines_and_strips_crlf_at_dispatch_boundary() {
        let mut buffer = b"{\"id\":1}\r".to_vec();
        assert!(complete_lines(&mut buffer).is_empty());
        buffer.extend_from_slice(b"\n{\"id\":2");
        let lines = complete_lines(&mut buffer);
        assert_eq!(lines, vec![b"{\"id\":1}\r\n".to_vec()]);
        assert_eq!(buffer, b"{\"id\":2");
    }

    #[test]
    fn oversized_permission_field_is_cancelled_before_storage() {
        let (broker, sent) = test_broker();
        let reader = AcpReader::for_test(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            Arc::clone(&broker),
        );
        let runtime = Arc::new(SessionRuntime::new());
        reader.dispatch_permission(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 62,
                "method": "session/request_permission",
                "params": {
                    "sessionId": "stub-session",
                    "title": "x".repeat(MAX_ACP_PERMISSION_FIELD_BYTES + 1),
                    "toolCall": {"toolCallId": "oversized"},
                    "options": [{"optionId": "allow", "name": "Allow once", "kind": "allow_once"}]
                }
            }),
            &runtime,
            None,
        );
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, 62);
        assert_eq!(sent[0].1["outcome"]["outcome"], "cancelled");
        assert_eq!(broker.pending_len(), 0);
    }

    #[test]
    fn oversized_unterminated_line_is_dropped_and_reported() {
        let (broker, _) = test_broker();
        let mut reader = AcpReader::for_test(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            broker,
        );
        let runtime = Arc::new(SessionRuntime::new());
        reader
            .feed(&vec![b'x'; MAX_ACP_PERMISSION_LINE_BYTES + 1], &runtime)
            .expect("oversized input is reported, not fatal to the reader");
        assert!(reader.buffer.is_empty());
    }

    #[test]
    fn detached_permission_is_queued_for_capable_reattach_and_removed_after_expiry() {
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::for_acp(
            "s.permission.queue".to_string(),
            None,
            Arc::clone(&broker),
        ));
        let first = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &first, true)
            .expect("first attach");
        first.track_with_agent_replay(
            "s.permission.queue",
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        runtime.detach_if_conn(first.id);

        let request = permission("queued");
        let pending = broker
            .register(7, request.clone(), &runtime)
            .expect("register");
        runtime.publish_agent_event(request, None);

        let second = ConnHandle::new(2);
        let outcome = runtime
            .try_attach_with_replay(None, &second, true)
            .expect("reattach");
        second.track_with_agent_replay(
            "s.permission.queue",
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        assert!(second.pull_events().iter().any(|event| matches!(
            event.envelope.event,
            SessionEvent::PermissionRequest { ref tool_call_id, .. } if tool_call_id == "queued"
        )));

        assert!(broker.expire("queued", &pending));
        let after_expiry = second.pull_events();
        assert!(
            after_expiry.iter().any(|event| matches!(
                event.envelope.event,
                SessionEvent::PermissionResolved { ref tool_call_id, .. } if tool_call_id == "queued"
            )),
            "expiry must tell the attached client the card is gone: {after_expiry:?}"
        );
        assert!(
            !after_expiry.iter().any(|event| matches!(
                event.envelope.event,
                SessionEvent::PermissionRequest { .. }
            )),
            "expiry must not re-deliver the request: {after_expiry:?}"
        );
    }

    #[test]
    fn detached_permission_expiry_is_not_replayed_on_later_reattach() {
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::for_acp(
            "s.permission.expired".to_string(),
            None,
            Arc::clone(&broker),
        ));
        let request = permission("expired-detached");
        let pending = broker
            .register(8, request.clone(), &runtime)
            .expect("register");
        runtime.publish_agent_event(request, None);
        assert!(broker.expire("expired-detached", &pending));

        let conn = ConnHandle::new(3);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("reattach");
        conn.track_with_agent_replay(
            "s.permission.expired",
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        let events = conn.pull_events();
        assert!(
            !events.iter().any(|event| matches!(
                event.envelope.event,
                SessionEvent::PermissionRequest { .. }
            )),
            "expired permission must not replay as a request: {events:?}"
        );
    }

    #[test]
    fn reattach_reemits_the_stored_session_manifest() {
        let (broker, _) = test_broker();
        let runtime =
            SessionRuntime::for_acp("s.manifest.reattach".to_string(), None, Arc::clone(&broker));
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("grok".to_string()),
            current_model_id: Some("grok-4.6".to_string()),
            models: Vec::new(),
            modes: None,
        });

        let first = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &first, true)
            .expect("attach");
        first.track_with_agent_replay(
            "s.manifest.reattach",
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        let first_events = first.pull_events();
        assert!(
            first_events.iter().any(|event| matches!(
                event.envelope.event,
                SessionEvent::SessionManifest {
                    ref current_model_id,
                    ..
                } if current_model_id.as_deref() == Some("grok-4.6")
            )),
            "first attach must deliver the stored manifest: {first_events:?}"
        );

        runtime.detach_if_conn(first.id);
        let second = ConnHandle::new(2);
        let outcome = runtime
            .try_attach_with_replay(None, &second, true)
            .expect("reattach");
        second.track_with_agent_replay(
            "s.manifest.reattach",
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        let second_events = second.pull_events();
        assert!(
            second_events.iter().any(|event| matches!(
                event.envelope.event,
                SessionEvent::SessionManifest {
                    ref current_model_id,
                    ..
                } if current_model_id.as_deref() == Some("grok-4.6")
            )),
            "reattach must re-emit the stored manifest: {second_events:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn reader_finish_releases_terminals_left_by_a_dead_agent() {
        use super::AcpHost;
        use crate::process_tree::JobObject;
        let cwd = std::env::temp_dir().join(format!(
            "devboule-acp-finish-cwd-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let runtime = std::env::temp_dir().join(format!(
            "devboule-acp-finish-rt-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&cwd).expect("cwd");
        std::fs::create_dir_all(&runtime).expect("runtime");
        let host = AcpHost::new(
            cwd.clone(),
            runtime.clone(),
            Arc::new(JobObject::new().expect("job")),
        );
        host.set_session_id("stub-session".to_string());
        let (broker, _) = test_broker();
        let session_runtime = Arc::new(SessionRuntime::for_acp(
            "stub-session".to_string(),
            None,
            Arc::clone(&broker),
        ));
        host.bind_permission_gate(&broker, &session_runtime);
        let allow_broker = Arc::clone(&broker);
        let allow = thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(id) = allow_broker.pending_ids().into_iter().next() {
                    let _ = allow_broker.respond(&id, PermissionOutcome::AllowOnce);
                    return;
                }
                if std::time::Instant::now() >= deadline {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        host.test_create_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "command": "ping.exe",
            "args": ["-t", "127.0.0.1"]
        }))
        .expect("create lingering terminal");
        let _ = allow.join();
        assert_eq!(host.live_terminal_count(), 1);
        let (broker, _) = test_broker();
        let mut reader = AcpReader::for_test_on_host(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            broker,
            Arc::clone(&host),
        );
        let session_runtime = Arc::new(SessionRuntime::new());
        reader.finish(&session_runtime);
        assert_eq!(
            host.live_terminal_count(),
            0,
            "EOF must shut down ACP terminals the dead agent left behind"
        );
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(runtime);
    }

    #[cfg(windows)]
    #[test]
    fn killer_does_not_block_on_a_full_agent_stdin() {
        use super::{AcpHost, AcpKiller, AcpTransport};
        use crate::process_tree::JobObject;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let mut child = Command::new("ping.exe")
            .args(["-n", "99999", "127.0.0.1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("ping");
        let stdin = child.stdin.take().expect("stdin");
        let cwd = std::env::temp_dir();
        let host = AcpHost::new(cwd.clone(), cwd, Arc::new(JobObject::new().expect("job")));
        let transport = Arc::new(AcpTransport::new(stdin, host));
        transport.set_session_id("stub-session".to_string());
        let runtime = Arc::new(SessionRuntime::new());
        transport
            .permission_broker
            .register(1, permission("stuck-kill"), &runtime)
            .expect("pending permission so close must write stdin");
        let filler = {
            let transport = Arc::clone(&transport);
            thread::spawn(move || {
                let blob = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/prompt",
                    "params": { "pad": "x".repeat(4096) }
                });
                for _ in 0..64 {
                    if transport.send_line(&blob).is_err() {
                        break;
                    }
                }
            })
        };
        thread::sleep(Duration::from_millis(200));
        let mut killer = AcpKiller {
            process: Arc::new(Mutex::new(child)),
            permission_broker: Arc::clone(&transport.permission_broker),
            transport: Arc::clone(&transport),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let started = Instant::now();
        thread::spawn(move || {
            killer.kill();
            let _ = done_tx.send(started.elapsed());
        });
        let elapsed = done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("kill blocked waiting on agent stdin");
        let _ = filler.join();
        assert!(
            elapsed < Duration::from_secs(2),
            "kill blocked for {elapsed:?} waiting on agent stdin"
        );
    }

    #[cfg(windows)]
    #[test]
    fn kill_unblocks_a_pending_terminal_create_gate() {
        use super::{AcpHost, AcpKiller, AcpTransport};
        use crate::process_tree::JobObject;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::Instant;
        let mut child = Command::new("ping.exe")
            .args(["-n", "30", "127.0.0.1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("ping");
        let stdin = child.stdin.take().expect("stdin");
        let cwd = std::env::temp_dir().join(format!(
            "devboule-acp-kill-gate-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&cwd).expect("cwd");
        let host = AcpHost::new(
            cwd.clone(),
            cwd.clone(),
            Arc::new(JobObject::new().expect("job")),
        );
        host.set_session_id("stub-session".to_string());
        let transport = Arc::new(AcpTransport::new(stdin, Arc::clone(&host)));
        let runtime = SessionRuntime::for_acp(
            "stub-session".to_string(),
            None,
            Arc::clone(&transport.permission_broker),
        );
        host.bind_permission_gate(&transport.permission_broker, &runtime);
        let create_host = Arc::clone(&host);
        let create = thread::spawn(move || {
            create_host.test_create_terminal(serde_json::json!({
                "sessionId": "stub-session",
                "command": "cmd.exe",
                "args": ["/c", "exit"]
            }))
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while transport.permission_broker.pending_len() == 0 {
            if create.is_finished() {
                panic!(
                    "create finished without a pending gate: {:?}",
                    create.join()
                );
            }
            if Instant::now() >= deadline {
                panic!("create never reached the terminal permission gate");
            }
            thread::sleep(Duration::from_millis(5));
        }
        let mut killer = AcpKiller {
            process: Arc::new(Mutex::new(child)),
            permission_broker: Arc::clone(&transport.permission_broker),
            transport: Arc::clone(&transport),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let started = Instant::now();
        killer.kill();
        let result = create.join().expect("create thread");
        let elapsed = started.elapsed();
        let _ = std::fs::remove_dir_all(&cwd);
        let error = result.expect_err("kill must deny the pending terminal create");
        assert!(
            elapsed < Duration::from_secs(2),
            "kill left the terminal gate blocked for {elapsed:?}"
        );
        assert_eq!(error.code, -32001);
        assert_eq!(error.message, "the user denied this command");
        assert_eq!(host.spawned_count(), 0);
    }

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

    fn event_kinds(conn: &ConnHandle) -> Vec<&'static str> {
        conn.pull_events()
            .into_iter()
            .map(|event| match event.envelope.event {
                SessionEvent::AgentFinished { .. } => "finished",
                SessionEvent::AgentError { .. } => "error",
                SessionEvent::PermissionRequest { .. } => "permission",
                SessionEvent::PermissionResolved { .. } => "permission_resolved",
                SessionEvent::AgentThought { .. } => "thought",
                SessionEvent::Snapshot { .. } => "snapshot",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn late_prompt_result_after_cancel_is_not_a_second_outcome() {
        let (broker, _) = test_broker();
        let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
        let mut reader = AcpReader::for_test(
            Arc::new(Mutex::new(HashSet::from([7u64]))),
            "stub-session".to_string(),
            broker,
        );
        reader.turn.start_prompt(7);
        reader.turn.abandon_live_prompt();
        let _ = event_kinds(&conn);
        reader
            .feed(
                br#"{"jsonrpc":"2.0","id":7,"result":{"stopReason":"end_turn"}}"#
                    .as_ref()
                    .iter()
                    .copied()
                    .chain(std::iter::once(b'\n'))
                    .collect::<Vec<_>>()
                    .as_slice(),
                &runtime,
            )
            .expect("feed");
        let kinds = event_kinds(&conn);
        assert!(
            !kinds.contains(&"finished"),
            "timed-out turn published AgentFinished: {kinds:?}"
        );
    }

    #[test]
    fn permission_after_turn_cancel_is_not_shown_to_the_user() {
        let (broker, sent) = test_broker();
        let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
        let reader = AcpReader::for_test(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            Arc::clone(&broker),
        );
        reader.turn.start_prompt(1);
        reader.turn.abandon_live_prompt();
        let _ = event_kinds(&conn);
        reader.dispatch_permission(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 88,
                "method": "session/request_permission",
                "params": {
                    "sessionId": "stub-session",
                    "title": "Run command",
                    "toolCall": {"toolCallId": "late-perm"},
                    "options": [{"optionId": "allow", "name": "Allow once", "kind": "allow_once"}]
                }
            }),
            &runtime,
            None,
        );
        let kinds = event_kinds(&conn);
        assert_eq!(broker.pending_len(), 0, "late permission stayed pending");
        assert!(
            !kinds.contains(&"permission"),
            "cancelled turn published a permission prompt: {kinds:?}"
        );
        let sent = sent.lock().expect("sent lock");
        assert!(
            sent.iter()
                .any(|(id, result)| *id == 88 && result["outcome"]["outcome"] == "cancelled"),
            "agent was not told the late permission was cancelled: {sent:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn cancel_closure_does_not_keep_transport_alive() {
        use super::{AcpHost, AcpTransport};
        use crate::process_tree::JobObject;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        let mut child = Command::new("ping.exe")
            .args(["-n", "2", "127.0.0.1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("ping");
        let stdin = child.stdin.take().expect("stdin");
        let cwd = std::env::temp_dir();
        let host = AcpHost::new(cwd.clone(), cwd, Arc::new(JobObject::new().expect("job")));
        let transport = Arc::new(AcpTransport::new(stdin, host));
        transport.bind_turn();
        let weak = Arc::downgrade(&transport);
        drop(transport);
        let leaked = weak.upgrade();
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            leaked.is_none(),
            "TurnWatch cancel closure kept AcpTransport alive after the session dropped it"
        );
    }

    #[cfg(windows)]
    #[test]
    fn reader_keeps_dispatching_while_a_host_call_is_blocked() {
        use super::{AcpHost, AcpTransport};
        use crate::process_tree::JobObject;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let cwd = std::env::temp_dir().join(format!(
            "devboule-acp-e-cwd-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let runtime_dir = std::env::temp_dir().join(format!(
            "devboule-acp-e-rt-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&cwd).expect("cwd");
        std::fs::create_dir_all(&runtime_dir).expect("runtime");
        let host = AcpHost::new(
            cwd.clone(),
            runtime_dir.clone(),
            Arc::new(JobObject::new().expect("job")),
        );
        host.set_session_id("stub-session".to_string());
        let gap = Arc::new(Barrier::new(2));
        host.set_create_gap(Arc::clone(&gap));
        let mut child = Command::new("ping.exe")
            .args(["-n", "30", "127.0.0.1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("ping");
        let stdin = child.stdin.take().expect("stdin");
        let transport = Arc::new(AcpTransport::new(stdin, Arc::clone(&host)));
        transport.set_session_id("stub-session".to_string());
        let (broker, _) = test_broker();
        let (session_runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
        let mut reader = AcpReader::for_test_with_transport(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            Arc::clone(&broker),
            Arc::clone(&host),
            transport,
        );
        let create = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "terminal/create",
            "params": {
                "sessionId": "stub-session",
                "command": "cmd.exe",
                "args": ["/c", "exit"]
            }
        });
        let thought = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "stub-session",
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "content": {"type": "text", "text": "still-alive"}
                }
            }
        });
        let mut bytes = serde_json::to_vec(&create).expect("create line");
        bytes.push(b'\n');
        bytes.extend(serde_json::to_vec(&thought).expect("thought line"));
        bytes.push(b'\n');
        let feed_runtime = Arc::clone(&session_runtime);
        let feed_thread = thread::spawn(move || reader.feed(&bytes, &feed_runtime));
        let allow_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(id) = broker.pending_ids().into_iter().next() {
                broker
                    .respond(&id, PermissionOutcome::AllowOnce)
                    .expect("allow blocked terminal/create so it can hit the create gap");
                break;
            }
            if Instant::now() >= allow_deadline {
                panic!("terminal/create never registered a host permission");
            }
            thread::sleep(Duration::from_millis(5));
        }
        let deadline = Instant::now() + Duration::from_millis(500);
        let mut saw_thought = false;
        while Instant::now() < deadline {
            if conn.pull_events().iter().any(|event| {
                matches!(
                    &event.envelope.event,
                    SessionEvent::AgentThought { text, .. } if text == "still-alive"
                )
            }) {
                saw_thought = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        gap.wait();
        feed_thread.join().expect("feed thread").expect("feed");
        host.shutdown();
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(runtime_dir);
        assert!(
            saw_thought,
            "reader stayed blocked on terminal/create and never dispatched the next update"
        );
    }
}
