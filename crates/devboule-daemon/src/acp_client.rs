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
    SwitchControlShape,
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
use crate::profile_delivery::ProfileDelivery;

const COMMAND_ENV: &str = "DEVBOULE_ACP_COMMAND";

/// Serialises the tests that write the ACP override environment.
///
/// Those variables are process-global and Rust runs tests as threads in one
/// process, so four tests setting and clearing them concurrently corrupt each
/// other. Measured 2026-09-17: a resume test failed asserting "cannot be
/// resumed while its process is running" and got "is not an ACP agent" —
/// another test had cleared the command between its own set and its act.
/// Every test that writes either variable holds this for the whole span.
#[cfg(test)]
pub(crate) fn lock_acp_env() -> std::sync::MutexGuard<'static, ()> {
    static ACP_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());
    ACP_ENV
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
/// Test/direct-command counterpart to [`COMMAND_ENV`]. A direct command has
/// no catalog row to identify it; tests set this to the stub provider id so a
/// later resume can still exercise the named-provider path.
const COMMAND_PROVIDER_ENV: &str = "DEVBOULE_ACP_PROVIDER_ID";

/// Silence after `session/prompt` with no inbound traffic and no outstanding
/// client work. Grok stays mute instead of erroring when `terminal` is missing.
pub const ACP_TURN_SILENCE: Duration = Duration::from_secs(60);
const TURN_TIMEOUT_ENV: &str = "DEVBOULE_ACP_TURN_TIMEOUT_MS";
const MAX_ACP_PERMISSION_LINE_BYTES: usize = 256 * 1024;
pub(crate) const ACP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_TIMEOUT_ENV: &str = "DEVBOULE_ACP_RESPONSE_TIMEOUT_MS";

/// The bound on the provider's **first** answer — its `initialize` reply.
///
/// Every other awaited rpc is agent work on a child that has already spoken,
/// so fifteen seconds is the same patience the rest of the daemon carries.
/// The first one is not agent work: it is the provider's own startup, and for
/// an `npx` wrapper that means a package download the daemon can neither see
/// nor hurry. Measured 2026-09-21 against
/// `npx -y @agentclientprotocol/claude-agent-acp@0.79.0` by hand: 2.4 s warm,
/// 14.1 s with a fresh npm cache, and 20.7 s on a run whose cache was already
/// warm — the same command, twenty seconds of startup with nothing on stderr.
/// The fifteen-second bound is below that spread, and it was read by the
/// committente as *"the ACP did not answer within 15s"* for an agent that was
/// only downloading. The two minutes are **5.8×** the slowest start measured:
/// the margin a machine several times slower than this one gets, and above
/// every wait the product's own family documents (60 s for a client rpc and a
/// run's start, 65 s for the phone's history sync, 90 s to list importable
/// sessions). That is a declared **product judgement, not a measurement of its
/// own** — no cold start was timed through the daemon — and it is not the
/// whole window a person waits: the app's client library carries the road's
/// own budget on top of it (`crate::client::SESSION_RESUME_RPC_TIMEOUT`, which
/// is two of these bounds: a resume the provider refuses is answered by a
/// recovery, and that is a second provider startup), which is where a
/// `src/lib/tauri.ts` read finds nothing. The window that matters is the one in
/// the layer that waits.
pub(crate) const ACP_FIRST_RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);
const FIRST_RESPONSE_TIMEOUT_ENV: &str = "DEVBOULE_ACP_FIRST_RESPONSE_TIMEOUT_MS";

type AcpModeResponses = Arc<Mutex<HashMap<u64, Sender<Result<(), String>>>>>;

fn turn_silence() -> Duration {
    std::env::var(TURN_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .map(Duration::from_millis)
        .filter(|duration| !duration.is_zero())
        .unwrap_or(ACP_TURN_SILENCE)
}

/// The bound one awaited response carries — the handshake rpcs and the
/// creation-time confirm alike. Tests shorten it through the environment;
/// production gets the fifteen seconds every other awaited rpc in the
/// daemon carries.
fn response_timeout() -> Duration {
    std::env::var(RESPONSE_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .map(Duration::from_millis)
        .filter(|duration| !duration.is_zero())
        .unwrap_or(ACP_RESPONSE_TIMEOUT)
}

/// See [`ACP_FIRST_RESPONSE_TIMEOUT`]. Read through its own variable so a test
/// that needs a mute provider bounds that wait without touching the fifteen
/// seconds every other awaited rpc keeps.
fn first_response_timeout() -> Duration {
    std::env::var(FIRST_RESPONSE_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .map(Duration::from_millis)
        .filter(|duration| !duration.is_zero())
        .unwrap_or(ACP_FIRST_RESPONSE_TIMEOUT)
}

/// Whether the child's stdout can be read without blocking.
///
/// `Ok(true)` — bytes are in the pipe, so one `fill_buf` read returns
/// immediately. `Ok(false)` — the pipe is open but empty, so wait. `Err` —
/// the pipe is broken or unreadable: the child is gone (or going), and the
/// read must surface that as the EOF sentence instead of the deadline.
#[cfg(windows)]
fn stdout_has_bytes_or_died(reader: &BufReader<ChildStdout>) -> Result<bool, ()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;
    let mut available: u32 = 0;
    let ok = unsafe {
        PeekNamedPipe(
            reader.get_ref().as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    };
    if ok != 0 {
        Ok(available > 0)
    } else {
        Err(())
    }
}

/// Where there is no pipe to peek, this question has no non-blocking answer.
/// `Ok(true)` means one thing only: *proceed into the fill* — and on this
/// platform that fill blocks until bytes arrive or the child dies, with the
/// deadline unable to reach it. It must never be read as "bytes are
/// available"; [`read_line_bounded`]'s platform paragraph states what bound
/// does and does not exist here.
#[cfg(not(windows))]
fn stdout_blocks_until_bytes(_reader: &BufReader<ChildStdout>) -> Result<bool, ()> {
    Ok(true)
}

/// Read one newline-terminated line, bounded by `deadline` — on Windows.
///
/// `BufRead::read_line` on a child's stdout has no timeout of its own, and
/// an agent that takes the request off the wire and never answers it would
/// hold the creation — the child, the reservation, the journal row and the
/// caller's tool call — forever (the re-audit's P2-2). On Windows the read
/// is assembled from non-blocking pieces: bytes already in the `BufReader`
/// are consumed without I/O, the pipe is peeked before every fill, and —
/// the re-audit's P2-3 — the deadline is checked at the top of every
/// iteration, so an agent that keeps the pipe non-empty without a newline
/// is refused as boundedly as a mute one, and the line never grows past
/// [`MAX_ACP_PERMISSION_LINE_BYTES`]. A mute or dribbling agent becomes an
/// `Io` refusal naming the wait; a dead agent becomes the same EOF sentence
/// the plain read produced.
///
/// **On every other platform this read is not bounded.** There is no pipe
/// peek in std to poll a child's stdout against a deadline, and no
/// non-blocking mode without a libc this crate does not carry, so the
/// deadline has no mechanism to act through: `deadline` is accepted to keep
/// one call shape and is deliberately not honoured there. An agent that
/// never answers — or answers in bytes that never form a newline — holds
/// the creation on such a platform exactly the way the pre-fix read did.
/// Windows is the only target this daemon is built and tested on; a
/// platform added later must either give this function a real poll or keep
/// this paragraph telling the truth.
fn read_line_bounded(
    reader: &mut BufReader<ChildStdout>,
    deadline: Instant,
    budget: Duration,
) -> Result<String, WireError> {
    let mut line: Vec<u8> = Vec::new();
    loop {
        // The deadline is consulted on every iteration, not only when the
        // peek reports the pipe quiet (the re-audit's P2-3): a dribbler
        // that keeps bytes flowing without a newline must hit the same
        // bound a mute agent does.
        if Instant::now() >= deadline {
            return Err(WireError::new(
                ErrorCode::Io,
                format!(
                    "the ACP agent did not answer within {}s; the creation is refused rather than awaited without end",
                    budget.as_secs()
                ),
            ));
        }
        if line.len() > MAX_ACP_PERMISSION_LINE_BYTES {
            return Err(WireError::new(
                ErrorCode::Io,
                format!(
                    "the ACP agent wrote more than {} bytes without a newline; the creation is refused rather than buffered without end",
                    MAX_ACP_PERMISSION_LINE_BYTES
                ),
            ));
        }
        let buffered = reader.buffer();
        if let Some(pos) = buffered.iter().position(|byte| *byte == b'\n') {
            line.extend_from_slice(&buffered[..=pos]);
            reader.consume(pos + 1);
            return Ok(String::from_utf8_lossy(&line).into_owned());
        }
        line.extend_from_slice(buffered);
        reader.consume(buffered.len());
        // The Windows name asks what the peek sees; the other-platform name
        // states that the following fill is the blocking step. Two names,
        // because the honest answer differs per platform.
        #[cfg(windows)]
        let peek = stdout_has_bytes_or_died(reader);
        #[cfg(not(windows))]
        let peek = stdout_blocks_until_bytes(reader);
        match peek {
            Ok(true) => match reader.fill_buf() {
                Ok(bytes) => {
                    if bytes.is_empty() {
                        return Err(WireError::new(
                            ErrorCode::Io,
                            "ACP agent closed stdout before the answer arrived.",
                        ));
                    }
                }
                Err(error) => return Err(acp_io_error(error)),
            },
            Ok(false) => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(()) => {
                return Err(WireError::new(
                    ErrorCode::Io,
                    "ACP agent closed stdout before the answer arrived.",
                ));
            }
        }
    }
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
            publish_turn_finished(&runtime, "cancelled");
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

/// The id paired with [`COMMAND_ENV`] becomes a journal row's `provider` on a
/// session whose `kind` is ACP, so it must not name a native family. The ids
/// are the impls' own ([`native_family_ids`]), compared the way the id's
/// readers compare them — case-insensitively. Refused rather than stripped: a
/// direct command has no catalog row to fall back to, so dropping the id
/// would leave the session this variable exists to identify unnamed on its
/// next resume, hiding the misconfiguration instead of reporting it.
fn refuse_native_override_id(id: Option<&str>) -> Result<(), WireError> {
    let Some(id) = id else {
        return Ok(());
    };
    match crate::session::native_family_ids()
        .iter()
        .find(|native| native.eq_ignore_ascii_case(id))
    {
        None => Ok(()),
        Some(native) => Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!(
                "{COMMAND_PROVIDER_ENV}={id} names the native provider '{native}'; the \
                 {COMMAND_ENV} override runs on the ACP road and cannot claim another \
                 family's identity. Run the command under its own provider id instead."
            ),
        )),
    }
}

/// The PtyCommand for a catalog ACP agent the picker picked by default: its
/// argv, its id, and the same spawn PATH override the named route applies, so
/// an agent found through a registry folder launches with its folder visible
/// whichever route picked it.
fn catalog_acp_command(
    agent: crate::provider_catalog::InstalledAgent,
    cwd: std::path::PathBuf,
) -> PtyCommand {
    let id = agent.id.to_string();
    let mut argv = agent
        .acp_command
        .expect("an ACP-capable catalog entry has an ACP command");
    let program = argv.remove(0);
    PtyCommand::new(
        program,
        argv,
        cwd,
        agent.spawn_path_env.into_iter().collect(),
    )
    .with_provider_id(id)
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
            refuse_native_override_id(provider_id.as_deref())?;
            let argv = serde_json::from_str(&argv).map_err(|error| {
                WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("{COMMAND_ENV} must be a non-empty JSON string array: {error}"),
                )
            })?;
            (provider_id, argv)
        }
        Err(_) => {
            let agent = crate::provider_catalog::first_acp_available().ok_or_else(|| {
                WireError::new(
                    ErrorCode::Io,
                    format!(
                        "No ACP-capable agent was found on PATH. Set {COMMAND_ENV} to a non-empty JSON string array to choose an ACP command explicitly."
                    ),
                )
            })?;
            let command = catalog_acp_command(agent, cwd.clone());
            return Ok(command);
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
    // A user row is the named road's own source: its command and env are
    // explicit, so it resolves here, before the PATH/CDN walk — a provider
    // the user declared must not depend on either. The row comes from the
    // live registry snapshot, the same one the profile lookup answers from,
    // so a row that resolves here is exactly a row that was validated and
    // swapped in (pass 2e step 2: the rows ride the same road as a catalog
    // row; nothing here learns a new name).
    if let Some(row) = crate::session::catalog_registry().user_row_for(id) {
        // Validation refuses a row without a command, so this arm is
        // unreachable for a live row; it refuses instead of unwrapping
        // because a data path must not panic the daemon.
        let Some(mut argv) = row.command else {
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Provider '{id}' has no command to spawn."),
            ));
        };
        let program = argv.remove(0);
        let env: Vec<(String, String)> = row.env.unwrap_or_default().into_iter().collect();
        return Ok(PtyCommand::new(program, argv, cwd, env).with_provider_id(id.to_string()));
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
    Ok(PtyCommand::new(
        program,
        argv,
        cwd,
        agent.spawn_path_env.into_iter().collect(),
    )
    .with_provider_id(id.to_string()))
}

/// Spawn the ACP peer directly, complete initialize + session/new, and return
/// adapters that the ordinary session machinery can own.
pub(super) fn spawn_process(
    state: &Arc<ServerState>,
    command: PtyCommand,
    mcp: Option<McpLaunchConfig>,
    delivery: ProfileDelivery,
) -> Result<SpawnedSession, WireError> {
    spawn_process_with_load(state, command, None, mcp, delivery)
}

pub(super) fn spawn_process_resuming(
    state: &Arc<ServerState>,
    command: PtyCommand,
    peer_session_id: String,
    mcp: Option<McpLaunchConfig>,
) -> Result<SpawnedSession, WireError> {
    spawn_process_with_load(
        state,
        command,
        Some(peer_session_id),
        mcp,
        ProfileDelivery::none(),
    )
}

fn spawn_process_with_load(
    state: &Arc<ServerState>,
    command: PtyCommand,
    load_session_id: Option<String>,
    mcp: Option<McpLaunchConfig>,
    delivery: ProfileDelivery,
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
        // This agent's own fresh job; why no shared job is ever an
        // assignment target is stated once, at open_pty_session.
        if let Err(error) = process_job.assign(handle) {
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
    let (mut deferred, mut handshake, peer_session_id, agent_version) = match handshake(
        &transport,
        &mut reader,
        &command.cwd,
        command.provider_id.clone(),
        load_session_id.as_deref(),
        mcp.as_ref(),
        delivery.mode_id.as_deref(),
    ) {
        Ok(handshake) => handshake,
        Err(error) => {
            // A provider that died during its own startup — initialize,
            // session/new, session/load, set_mode — never became a session
            // (audit-2 §1): it is named as that, not as an I/O fault that
            // reads like a protocol problem. The status is read **before**
            // the teardown, through the same pre-kill poll the delivery arm
            // uses: a post-kill `try_wait` sees our own kill's cached status
            // and names every handshake failure an exit, including one the
            // daemon caused on a live agent. The arm also carries the code the
            // child left with: "the agent is gone" and "the agent is gone with
            // 1" are different facts, and only the second one can be acted on.
            let exited = provider_exit_before_teardown(&process);
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
            if let Some(code) = exited {
                // The boundary is *named* on top of the provider's own words:
                // a caller reads "provider exited during startup" and the
                // provider's last line stays in the message for the human.
                return Err(redact_handshake_error(
                    WireError::new(
                        error.code,
                        format!(
                            "provider exited during startup: {}{}",
                            bounded_excerpt(&error.message, MAX_HANDSHAKE_MESSAGE_BYTES),
                            exit_code_suffix(code)
                        ),
                    ),
                    &stderr_lines,
                    mcp.as_ref(),
                ));
            }
            return Err(redact_handshake_error(error, &stderr_lines, mcp.as_ref()));
        }
    };
    let session_id = transport.session_id();
    transport.set_model_switch_shape(handshake.shape);
    transport.set_prompt_capabilities(handshake.prompt_capabilities);
    transport.seed_manifest_from_event(handshake.event.as_ref());
    // The profile's delivery, judged now that the handshake has spoken: the
    // agent's declared modes and switch surfaces are what tell the daemon
    // what can be delivered, and after the handshake the daemon is not
    // guessing. A refusal tears the child down here — before it was ever a
    // session — instead of running a configuration the card did not name.
    let delivered_mode = handshake.event.as_ref().and_then(|event| match event {
        SessionEvent::SessionManifest {
            modes: Some(modes), ..
        } => Some(modes.current_mode_id.clone()),
        _ => None,
    });
    if let Err(error) = apply_profile_delivery(
        &transport,
        &mut reader,
        &mut deferred,
        &delivery,
        delivered_mode.as_deref(),
    ) {
        // The refusal teardown is the handshake's, line for line: the same
        // things are freed, and the same three behaviours hold — a provider
        // that died during the delivery is named as that, its last stderr
        // lines travel with the message, and the whole banner is redacted
        // before it leaves for the caller (the R2a audit's F5).
        //
        // Whether the provider died on its own is read **before** the
        // teardown: after the kill the exit status is ours, and the naming
        // would fire for every refusal, including an agent that answered
        // with an error and was then torn down.
        // The same pre-kill read the handshake arm makes: a live agent that
        // answered with an error must not be named as an exited provider.
        let exited = provider_exit_before_teardown(&process);
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
        if let Some(code) = exited {
            return Err(redact_handshake_error(
                WireError::new(
                    error.code,
                    format!(
                        "provider exited during startup: {}{}",
                        bounded_excerpt(&error.message, MAX_HANDSHAKE_MESSAGE_BYTES),
                        exit_code_suffix(code)
                    ),
                ),
                &stderr_lines,
                mcp.as_ref(),
            ));
        }
        return Err(redact_handshake_error(error, &stderr_lines, mcp.as_ref()));
    }
    // The manifest the session is born with is the handshake event, so it is
    // patched here with what the child actually took: a session created from
    // a profile starts showing the delivered model, not the handshake's
    // guess. The agent's own catalog pushes, if any follow, replay through
    // the deferred lines and update it again.
    if let Some(SessionEvent::SessionManifest {
        current_model_id,
        models,
        ..
    }) = handshake.event.as_mut()
    {
        if let Some(model_id) = delivery
            .model_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            *current_model_id = Some(model_id.to_string());
            if let Some(effort) = delivery
                .thinking_option_id
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                for model in models.iter_mut() {
                    if model.model_id == model_id {
                        model.current_effort = Some(effort.to_string());
                    }
                }
            }
        }
    }
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
        // The delivery was applied inside `spawn_process`, before this value
        // existed; nothing is left for the session reader to answer.
        pending_delivery: None,
        pending_codex_verify: None,
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

/// The switch requests one delivery put on the wire: the primary request's
/// id, and the follow-up the session reader would send once the primary
/// succeeded. The creation-time confirm walks both itself — its answers are
/// read on the spot, because no reader exists yet — while the runtime switch
/// leaves both to `dispatch_response` as before.
#[derive(Clone, Debug)]
struct SentSwitch {
    primary_id: u64,
    followup: Option<FollowupSwitch>,
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

    /// Reflect a creation-time confirmed switch in the remembered manifest.
    /// The runtime switch reaches the same state through
    /// `complete_vendor_switch`/`complete_config_switch`, which also publish
    /// to a live runtime; nothing is published here — the session does not
    /// exist yet — but the manifest the handshake seeded must not go on
    /// naming a model the child has already been switched away from.
    fn patch_manifest_current(&self, model_id: Option<String>, effort: Option<String>) {
        let Some(model_id) = model_id else {
            return;
        };
        let Some(SessionEvent::SessionManifest {
            provider_id,
            models,
            modes,
            ..
        }) = self.last_manifest()
        else {
            return;
        };
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
        self.remember_manifest(&SessionEvent::SessionManifest {
            provider_id,
            current_model_id: Some(model_id),
            models,
            modes,
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
            // The runtime switch's answers are dispatched by the session
            // reader (alternates, follow-ups, manifest re-emission). Only
            // the creation-time confirm walks the sent requests itself; see
            // `apply_profile_delivery`.
            self.set_requested_model(shape, model_id, requested_effort)?;
            return Ok(());
        }
        if let Some(effort) = requested_effort {
            self.set_requested_effort(shape, effort)?;
            return Ok(());
        }
        let current_model = self.transport.current_model_id().ok_or_else(|| {
            WireError::new(
                ErrorCode::InvalidRequest,
                "ACP provider has not reported a current model.",
            )
        })?;
        self.set_requested_model(shape, current_model, None)
            .map(|_sent| ())
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
    ) -> Result<SentSwitch, WireError> {
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
                let primary_id = self
                    .transport
                    .request_set_config_option(
                        &config.id,
                        &model_id,
                        ConfigControl::Model,
                        Some(model_id.clone()),
                        effort.clone(),
                        alternate,
                        followup.clone(),
                    )
                    .map_err(acp_io_error)?;
                Ok(SentSwitch {
                    primary_id,
                    followup,
                })
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
                let primary_id = self
                    .transport
                    .request_set_model(model_id, vendor_effort, alternate, followup.clone())
                    .map_err(acp_io_error)?;
                Ok(SentSwitch {
                    primary_id,
                    followup,
                })
            }
        }
    }

    fn set_requested_effort(
        &self,
        shape: ModelSwitchShape,
        effort: String,
    ) -> Result<SentSwitch, WireError> {
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
                let primary_id = self
                    .transport
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
                Ok(SentSwitch {
                    primary_id,
                    followup: None,
                })
            }
            None => {
                let model_id = current_model.ok_or_else(|| {
                    WireError::new(
                        ErrorCode::InvalidRequest,
                        "Cannot change effort before the provider reports its current model.",
                    )
                })?;
                let alternate = self.config_effort_alternate(&shape, &effort);
                let primary_id = self
                    .transport
                    .request_set_model(model_id, Some(effort), alternate, None)
                    .map_err(acp_io_error)?;
                Ok(SentSwitch {
                    primary_id,
                    followup: None,
                })
            }
        }
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

/// The creation-time delivery for one ACP child, run after the handshake:
/// this is the point where the agent's own declarations — its modes and its
/// model/effort switch surfaces — tell the daemon what can be delivered, and
/// after the handshake the daemon is not guessing. Everything that cannot be
/// delivered is refused here; a child that exists was delivered everything
/// its card printed.
///
/// Absent vocabulary and unknown id are two different refusals, per field: a
/// model sent to an agent that declares no switch surface is the absence
/// sentence, a model outside the agent's declared values is the mismatch
/// sentence, and the same split holds for the thinking option.
fn apply_profile_delivery(
    transport: &Arc<AcpTransport>,
    reader: &mut BufReader<ChildStdout>,
    deferred: &mut Vec<serde_json::Value>,
    delivery: &ProfileDelivery,
    delivered_mode: Option<&str>,
) -> Result<(), WireError> {
    // `autoAccept` is a constraint on which mode is delivered, not a value to
    // hand over: the delivered mode must be one the daemon's own broker
    // answers. An agent whose modes are provider-authored prose cannot have
    // the fact established, and an unestablishable permission fact is
    // refused, never waved through.
    if delivery.auto_accept {
        let mode_id = delivered_mode.ok_or_else(|| {
            WireError::new(
                ErrorCode::InvalidRequest,
                "the profile asks this agent to approve its own permission prompts, but the agent's handshake declared no modes the daemon can judge; the creation is refused",
            )
        })?;
        if !crate::provider_catalog::mode_is_auto_answered(mode_id) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "the profile asks this agent to approve its own permission prompts and also to start in mode '{mode_id}', which asks the human; the two contradict, so the creation is refused"
                ),
            ));
        }
    }
    if delivery.model_id.is_none() && delivery.thinking_option_id.is_none() {
        return Ok(());
    }
    let shape = transport.model_switch_shape();
    let Some(shape) = shape else {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "the profile names a model, but this agent's session declares no model or effort switch surface; the creation is refused rather than started on a different model",
        ));
    };
    if let Some(model_id) = delivery.model_id.as_deref() {
        validate_acp_model_choice(&shape.model, model_id)?;
    }
    if let Some(effort) = delivery.thinking_option_id.as_deref() {
        validate_acp_effort_choice(&shape.effort, delivery.model_id.as_deref(), effort)?;
    }
    // Deliver on the same wire the runtime switch uses, so the verbs and the
    // pending-switch bookkeeping are the proven ones. The send is
    // synchronous — and so is the **confirmation**: the response is read
    // here, the way the handshake reads its answers, because the session
    // reader that dispatches responses does not exist yet. A creation that
    // reported success now would promise a model the child may never run
    // (the R2a audit's F3): an agent that answers the switch with an error
    // refuses the creation instead.
    let switcher = AcpSwitcher {
        transport: Arc::clone(transport),
    };
    let requested_model = delivery
        .model_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let requested_effort = delivery
        .thinking_option_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let sent = if let Some(model_id) = &requested_model {
        switcher.set_requested_model(shape, model_id.clone(), requested_effort.clone())?
    } else if let Some(effort) = &requested_effort {
        switcher.set_requested_effort(shape, effort.clone())?
    } else {
        return Ok(());
    };
    confirm_switch(transport, reader, deferred, &sent, delivery)?;
    // What the card named is now what the transport reports, so the runtime
    // switcher starts from the delivered values rather than the handshake's
    // guess.
    if let Some(model_id) = &requested_model {
        transport.update_current_model_id(Some(model_id.clone()));
    }
    if let Some(effort) = &requested_effort {
        transport.update_current_effort(Some(effort.clone()));
    }
    transport.patch_manifest_current(requested_model, requested_effort);
    Ok(())
}

/// Whether the provider is gone before the teardown, and the code it left
/// with.
///
/// The read happens **before** the teardown: after the kill the exit status is
/// ours, and naming from a post-kill read would fire for every refusal,
/// including an agent that answered with an error and was then torn down —
/// which is exactly what the handshake arm's old post-kill read did (the
/// re-audit's P3-1 note). A child whose stdout the daemon just read EOF from is
/// on its way out: its exit becomes observable a beat after the pipe closes,
/// hence the short bounded poll rather than one `try_wait`.
///
/// `None` — still running (a live agent that answered an error, or one that is
/// merely slow). `Some(None)` — gone with no code to name (killed, or a status
/// this platform does not report as a code). `Some(Some(code))` — gone, and the
/// code is the provider's own last word about why.
fn provider_exit_before_teardown(process: &Arc<Mutex<std::process::Child>>) -> Option<Option<i32>> {
    for _ in 0..40 {
        let status = process
            .lock()
            .ok()
            .and_then(|mut process| process.try_wait().ok().flatten());
        if let Some(status) = status {
            return Some(status.code());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

/// The exit code as the sentence's own tail, or nothing when there is none.
/// A provider killed by a signal reports no code, and "exit code 0" would be a
/// lie about it: the absence is stated by saying nothing rather than by a
/// number nobody measured.
fn exit_code_suffix(code: Option<i32>) -> String {
    match code {
        Some(code) => format!(" (the child's exit code was {code})"),
        None => String::new(),
    }
}

/// The creation-time switch confirmation, primary and follow-up: each
/// response is read on the spot and its pending entries retired here,
/// because the reader that would dispatch them never sees this response. An
/// error answer — or a peer that dies waiting — is the creation's refusal.
///
/// (The paragraph sat on `provider_exit_before_teardown` until the recovery
/// slice: it describes this function, and the exit status it mentions is the
/// one the handshake arm reads.)
fn confirm_switch(
    transport: &Arc<AcpTransport>,
    reader: &mut BufReader<ChildStdout>,
    deferred: &mut Vec<serde_json::Value>,
    sent: &SentSwitch,
    delivery: &ProfileDelivery,
) -> Result<(), WireError> {
    confirm_one_switch(transport, reader, deferred, sent.primary_id, delivery)?;
    if let Some(followup) = &sent.followup {
        let followup_id = match &followup.request {
            SwitchRequest::Vendor { model_id, effort } => transport
                .request_set_model(model_id.clone(), effort.clone(), None, None)
                .map_err(acp_io_error)?,
            SwitchRequest::Config {
                config_id,
                value,
                control,
            } => transport
                .request_set_config_option(config_id, value, *control, None, None, None, None)
                .map_err(acp_io_error)?,
        };
        confirm_one_switch(transport, reader, deferred, followup_id, delivery)?;
    }
    Ok(())
}

fn confirm_one_switch(
    transport: &Arc<AcpTransport>,
    reader: &mut BufReader<ChildStdout>,
    deferred: &mut Vec<serde_json::Value>,
    id: u64,
    delivery: &ProfileDelivery,
) -> Result<(), WireError> {
    let response = read_response_envelope(transport, reader, id, deferred, response_timeout());
    transport.remove_pending_id(id);
    transport.remove_model_switch(id);
    let response = response?;
    // An error **object** is the agent's own answer to the switch: the card
    // promised what was delivered, the agent would not take it, so the
    // creation is refused rather than started on a different model. A
    // transport failure above keeps its `Io` code, because that is not a
    // refusal — it is a death or a broken pipe, and the naming downstream
    // (or the plain pipe error) has to be able to say so.
    if let Some(error) = response.get("error") {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!(
                "the agent refused the delivered model '{}' the card promised, so the creation is refused rather than started on a different model: {}",
                delivery.model_id.as_deref().unwrap_or(""),
                acp_request_error_message(error)
            ),
        ));
    }
    Ok(())
}

/// The model axis of the ACP refusal: absence of any declared surface is one
/// sentence, an id outside the agent's declared values is the other. An
/// empty declared list is no vocabulary — the agent's own answer to the
/// switch is then the confirmation, and this check refuses nothing.
pub(super) fn validate_acp_model_choice(
    shape: &SwitchControlShape,
    model_id: &str,
) -> Result<(), WireError> {
    let declared: Option<&Vec<String>> = match (shape.config.as_ref(), shape.vendor.as_ref()) {
        (Some(config), _) => Some(&config.values),
        (None, Some(vendor)) => Some(&vendor.values),
        (None, None) => None,
    };
    let Some(declared) = declared else {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "this agent declares no model switch surface; the profile names a model, so the creation is refused rather than started on a different model",
        ));
    };
    if !declared.is_empty() && !declared.iter().any(|value| value == model_id) {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("ACP model '{model_id}' is not among the model values this agent declares; the creation is refused rather than started on a different model"),
        ));
    }
    Ok(())
}

/// The thinking axis of the ACP refusal, with the same absence/mismatch
/// split. Vendor effort values are per model, so they judge the choice only
/// when the delivered model declares any; the config-option surface's values
/// are the option's own vocabulary.
pub(super) fn validate_acp_effort_choice(
    shape: &SwitchControlShape,
    model_id: Option<&str>,
    effort: &str,
) -> Result<(), WireError> {
    let effort_values: Option<Option<&Vec<String>>> = if let Some(config) = shape.config.as_ref() {
        Some(Some(&config.values))
    } else {
        shape.vendor.as_ref().map(|vendor| {
            model_id.and_then(|model_id| {
                vendor
                    .values_by_model
                    .iter()
                    .find(|(model, _)| model == model_id)
                    .map(|(_, values)| values)
            })
        })
    };
    let Some(effort_values) = effort_values else {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "this agent declares no thinking-option surface; the profile names one, so the creation is refused",
        ));
    };
    // Values the agent actually declares judge the choice; an empty list
    // is no vocabulary, and the agent's own answer to the switch is the
    // confirmation.
    if let Some(effort_values) = effort_values {
        if !effort_values.is_empty() && !effort_values.iter().any(|value| value == effort) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("ACP thinking option '{effort}' is not among the thinking options this agent declares; the creation is refused"),
            ));
        }
    }
    Ok(())
}

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
    let initialize = read_response(
        transport,
        reader,
        initialize_id,
        &mut deferred,
        first_response_timeout(),
    )?;
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
    let session = read_session_response(
        transport,
        reader,
        session_request_id,
        &mut deferred,
        load_session_id,
        response_timeout(),
    )?;
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
            let _ = read_response(
                transport,
                reader,
                request_id,
                &mut deferred,
                response_timeout(),
            )?;
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
    budget: Duration,
) -> Result<serde_json::Value, WireError> {
    let value = read_response_envelope(transport, reader, expected_id, deferred, budget)?;
    if let Some(error) = value.get("error") {
        return Err(WireError::new(
            ErrorCode::Io,
            acp_request_error_message(error),
        ));
    }
    Ok(value)
}

/// The handshake's session request (`session/new`, or `session/load` on the
/// resume road), read with the one classification the resume road needs.
/// This changes what the caller can tell apart, never what the user reads —
/// the message is spelled exactly as any other ACP error. Every other
/// failure (and every failure of a fresh session's `session/new`) answers
/// nothing about the far session and keeps the generic code.
fn read_session_response(
    transport: &AcpTransport,
    reader: &mut BufReader<ChildStdout>,
    expected_id: u64,
    deferred: &mut Vec<serde_json::Value>,
    load_session_id: Option<&str>,
    budget: Duration,
) -> Result<serde_json::Value, WireError> {
    let value = read_response_envelope(transport, reader, expected_id, deferred, budget)?;
    let Some(error) = value.get("error") else {
        return Ok(value);
    };
    let disowned = load_session_id.is_some_and(|peer| session_disown(error, peer));
    let code = if disowned {
        ErrorCode::SessionNotFound
    } else {
        ErrorCode::Io
    };
    Err(WireError::new(code, acp_request_error_message(error)))
}

/// The evidence standard for "the far agent does not have this session".
/// Two gates, both required. The **code**: `-32002` is the schema's word for
/// ANY missed resource — a file, a terminal; this daemon's own host answers
/// it for exactly those — so it narrows nothing by itself. The **name**: the
/// missed RESOURCE must be the session we asked to load — the `data.uri` the
/// schema gives for the miss, or the message tail after "resource not found"
/// — as a whole token, because a short handle inside a longer id or a file
/// name names nothing. An agent that echoes its request (session id
/// included) beside a missed file therefore fails the gate, exactly like an
/// agent that refuses without naming anything: the narrowing is allowed to
/// miss a real disown, never to invent one — the act it feeds records a
/// refusal, and a wrong record is a lie the row tells forever. Everything
/// else keeps the handle and the offer: a kept handle costs one wasted
/// click; a destroyed one cannot be recreated.
fn session_disown(error: &serde_json::Value, peer_session_id: &str) -> bool {
    if peer_session_id.is_empty() {
        return false;
    }
    let code = error.get("code").and_then(|value| {
        value
            .as_i64()
            // The number is what the code means, not its encoding: agents
            // have answered with a JSON float or a numeric string.
            .or_else(|| {
                value
                    .as_f64()
                    .filter(|number| number.fract() == 0.0)
                    .map(|number| number as i64)
            })
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
    });
    if code != Some(i64::from(super::acp_host::RESOURCE_NOT_FOUND)) {
        return false;
    }
    // The schema's own field for the missed resource. A uri that does not
    // name the session settles the question: whatever else the payload
    // echoes, the miss was of something else.
    if let Some(uri) = error
        .pointer("/data/uri")
        .and_then(serde_json::Value::as_str)
    {
        return uri_names_the_session(uri, peer_session_id);
    }
    error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .and_then(|message| {
            let lowered = message.to_ascii_lowercase();
            lowered
                .rfind("resource not found")
                .map(|at| &message[at + "resource not found".len()..])
        })
        .is_some_and(|tail| names_whole_token(tail, peer_session_id))
}

/// Whether the missed resource the uri names is the session itself: the uri
/// is the id, or the id is its final path segment. Anything longer — a
/// directory, a file beside the id — is another resource.
fn uri_names_the_session(uri: &str, peer_session_id: &str) -> bool {
    let trimmed = uri.trim_end_matches('/');
    trimmed == peer_session_id
        || trimmed
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|segment| segment == peer_session_id)
}

/// Whole-token containment: every occurrence of the handle must be bounded
/// by characters that cannot extend it, so `stub-session` inside
/// `stub-session-2` or `old-stub-session` names neither. A `.` binds too —
/// `stub-session.jsonl` is a file, not the session — which errs toward a
/// missed disown, the direction the asymmetry allows.
fn names_whole_token(haystack: &str, needle: &str) -> bool {
    let mut from = 0usize;
    while let Some(at) = haystack[from..].find(needle) {
        let start = from + at;
        let end = start + needle.len();
        let bounded = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !binds_handle(c))
            && haystack[end..]
                .chars()
                .next()
                .is_none_or(|c| !binds_handle(c));
        if bounded {
            return true;
        }
        from = start + 1;
    }
    false
}

fn binds_handle(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// The raw response naming `expected_id`: lines that name anything else are
/// deferred, a closed stdout and a malformed line are transport errors, and
/// an error **object** is returned as the value, because one caller — the
/// creation-time confirm — must tell an agent's refusal apart from a
/// transport failure. The handshake path goes through [`read_response`],
/// which converts the error object for it.
fn read_response_envelope(
    transport: &AcpTransport,
    reader: &mut BufReader<ChildStdout>,
    expected_id: u64,
    deferred: &mut Vec<serde_json::Value>,
    budget: Duration,
) -> Result<serde_json::Value, WireError> {
    // One deadline per awaited response: every read below is made against
    // it, so an rpc's own wait is the whole of what it is given (the
    // re-audit's P2-2). `budget` is the caller's: the first answer covers
    // the provider's startup, every later one the agent's own work.
    let deadline = Instant::now() + budget;
    loop {
        let line = read_line_bounded(reader, deadline, budget)?;
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
        return Ok(value);
    }
}

/// Format a JSON-RPC error object for a user-facing message. Agents embed
/// structured payloads in the error object (qwen carries `authMethods`);
/// the string `message` fields are what belong in a chat banner — `data`'s
/// first, because a provider that writes one puts its diagnosis there and
/// leaves only the JSON-RPC category in the envelope (grok's balance
/// exhaustion rode under `-32603 Internal error`). Without a string
/// message the code is reported bare — the object itself is never
/// serialized into the text.
fn acp_request_error_message(error: &serde_json::Value) -> String {
    let message = error
        .pointer("/data/message")
        .and_then(serde_json::Value::as_str)
        .filter(|message| !message.is_empty())
        .or_else(|| error.get("message").and_then(serde_json::Value::as_str));
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

/// How much of a startup failure's own words, and of its joined stderr, reach
/// the caller (audit-3 §4). A provider can answer a failed handshake with a
/// megabyte of output; this is a chat banner, not a log.
const MAX_HANDSHAKE_MESSAGE_BYTES: usize = 256;

/// See [`MAX_HANDSHAKE_MESSAGE_BYTES`].
const MAX_HANDSHAKE_STDERR_BYTES: usize = 1024;

/// The whole banner, not only its halves (audit-3 §4): the tool caller forwards
/// this string, and a provider that writes a hundred kilobytes to either half
/// must not push a transcript through a chat banner.
const MAX_HANDSHAKE_ERROR_BYTES: usize = 1024;

/// At most `limit` bytes of `text` — the `…` included (audit-3 S5D-03) — cut on a
/// character boundary when something was dropped.
fn bounded_excerpt(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    // The marker is part of the budget, not an extra on top of it: the caller
    // asked for `limit` bytes, so it is reserved inside that and the body is cut
    // at or below what is left.
    let mark = '…';
    if limit < mark.len_utf8() {
        return String::new();
    }
    let mut end = limit - mark.len_utf8();
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &text[..end], mark)
}

fn redact_handshake_error(
    error: WireError,
    stderr_lines: &[String],
    mcp: Option<&McpLaunchConfig>,
) -> WireError {
    if stderr_lines.is_empty() {
        return redact_mcp_error(error, mcp);
    }
    // Both halves are the provider's to choose, and the tool caller forwards
    // this string: an excerpt, not a transcript (audit-3 §4).
    let message = format!(
        "{} Agent stderr: {}",
        error.message,
        bounded_excerpt(&stderr_lines.join(" | "), MAX_HANDSHAKE_STDERR_BYTES)
    );
    let message = mcp
        .map(|config| config.redact_text(&message))
        .unwrap_or(message);
    WireError::new(
        error.code,
        bounded_excerpt(&message, MAX_HANDSHAKE_ERROR_BYTES),
    )
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
        let dir = crate::test_dirs::test_temp_dir("devboule-acp-for-test");
        Self::for_test_on_host(
            pending,
            session_id,
            permission_broker,
            AcpHost::new(dir.clone(), dir),
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

fn publish_turn_finished(runtime: &SessionRuntime, stop_reason: &str) {
    let _ = runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: stop_reason.to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
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
                let views = view_from_envelope_in(&value, &self.session_id, Some(self.host.cwd()));
                if !views.is_empty() {
                    let models_updated = value.get("method").and_then(serde_json::Value::as_str)
                        == Some("_x.ai/models/update");
                    for view in views {
                        let view = self.with_provider(view);
                        if models_updated && matches!(view, SessionEvent::SessionManifest { .. }) {
                            if let Some(transport) = &self.transport {
                                let shape =
                                    add_vendor_surface(transport.model_switch_shape(), &view);
                                transport.set_model_switch_shape(shape);
                            }
                        }
                        self.publish_at_seq(runtime, view, event_seq);
                    }
                } else if let Some(content_type) = unmodeled_content_kind(&value) {
                    // A text-bearing chunk whose content block is not text: the
                    // view returned nothing. Count and name it so a discarded
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
            publish_turn_finished(runtime, "error");
            return;
        }
        for view in view_from_envelope_in(value, &self.session_id, Some(self.host.cwd())) {
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
                is_chooser: None,
                // `unknown` is a placeholder, never a claim: the daemon
                // overwrites this with the session's stored origin at the
                // single place a permission request leaves for a subscriber.
                origin: devboule_protocol::SessionOrigin::unknown(),
                create_agent: None,
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
#[path = "acp_client_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "acp_client_delivery_tests.rs"]
mod delivery_tests;
