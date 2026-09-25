//! Codex app-server stdio transport for live agent sessions.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use devboule_protocol::{ErrorCode, NoticeSeverity, SessionEvent, WireError};
use serde_json::Value;

use super::permission_broker::{PermissionBroker, PermissionResponseError, PermissionSender};
use super::session_runtime::SessionRuntime;
use super::{
    write_child_stdin, ModelSwitcher, OutOfBandCommands, PtyCommand, ReaderDispatch, SessionKiller,
    SessionSteerer, SpawnedSession, StderrSource, StdioWaitableChild, TurnToken,
};
use crate::attachment_store::AttachmentStore;
use crate::codex_commands::{Answer, CodexCommands};
use crate::codex_goals::Goals;
use crate::codex_view::{
    catalog_from_response, mode_values, thread_mode_values, validate_mode, CodexCatalog,
    CodexState, CodexStdout, MAX_LINE_BYTES,
};
use crate::paths::RuntimePaths;
use crate::process_tree::{JobObject, ProcessHandle};
use crate::profile_delivery::ProfileDelivery;
use crate::provider_features::FAST_MODE_FEATURE;
use crate::server::ServerState;

/// The `serviceTier` value Codex's fast mode is spelled by. Paseo's
/// `turn/start` parameter of the same name takes `"fast"`, and no other value
/// is ever written here.
const SERVICE_TIER_FAST: &str = "fast";

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
    Ok(PtyCommand::new(
        program,
        argv,
        cwd,
        agent.spawn_path_env.into_iter().collect(),
    )
    .with_provider_id("codex"))
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
    // A stored feature with no frame here is refused before a process exists,
    // for the reason every family now gives: the card named it, and this client
    // sends `model`, `effort`, `approvalPolicy`, `sandboxPolicy` and
    // `serviceTier` and nothing else. The model gate is this family's own
    // `serviceTier`, read from the same table the form drew (`fastMode`).
    crate::profile_delivery::refuse_undeclared(
        &crate::provider_features::codex_declarations(),
        delivery.model_id.as_deref(),
        &delivery.features,
    )?;
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
pub(crate) fn seed_model_and_effort(
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

/// The profile's fast-mode tick, seeded as the service tier every later
/// `turn/start` carries. Called after [`seed_model_and_effort`] so the model
/// the gate is read against is the model the child runs.
///
/// `false` seeds nothing: the tier has no "off" spelling on this wire, and the
/// provider's default is the same state as no tick. A `true` for a model the
/// table does not list was refused by `validate_delivery` before this child
/// existed, so reaching here with one is a bug rather than a value to handle —
/// and it is still refused, because silently starting unflagged is the exact
/// silence the card rule forbids.
pub(crate) fn seed_fast_mode(
    state: &Arc<CodexState>,
    delivery: &ProfileDelivery,
) -> Result<(), WireError> {
    let Some(tick) = crate::profile_delivery::toggle_value(&delivery.features, FAST_MODE_FEATURE)?
    else {
        return Ok(());
    };
    if !tick {
        return Ok(());
    }
    // The delivered model, which `validate_delivery` judged and
    // `seed_model_and_effort` just set: the state's own current model is not
    // read, because it is the same string by then and borrowing it out of a
    // temporary would be a lifetime bug wearing a normal-looking line.
    let Some(model) = delivery
        .model_id
        .as_deref()
        .filter(|model| !model.is_empty())
    else {
        // No named model: this daemon has no fact about the provider's own
        // default to gate a flag on, so a fast tick is refused rather than
        // sent blind against a tier nobody named.
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "the profile asks Codex to run fast and names no model to check that against; the creation is refused",
        ));
    };
    let offered = crate::provider_features::codex_declarations()
        .iter()
        .filter(|row| row.id == FAST_MODE_FEATURE)
        .any(|row| crate::provider_features::offered_for(row, model));
    if !offered {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!(
                "the profile asks Codex to run fast on model '{model}', which does not carry that feature; the creation is refused rather than started without it",
            ),
        ));
    }
    state.set_service_tier(Some(SERVICE_TIER_FAST));
    Ok(())
}

/// The Codex carrier seam (S4 shape, S6 body — the provider-trait signatures
/// verbatim, so adoption is a move): the broker's server rides the app-server
/// launch line as `-c mcp_servers.<name>.url=...` overrides, with the bearer
/// named by `bearer_token_env_var` and carried as child **env**
/// (`DEVBOULE_MCP_TOKEN`). The token itself never rides argv — the S6
/// argv-token-free assertion pins this; the URL is not a secret. No
/// `CODEX_HOME` redirect: a per-session home carries no `auth.json` (measured:
/// every turn answers 401 "Missing bearer or basic authentication"), and the
/// rollout this client resumes lives under the home that wrote it, which the
/// startup sweep then deletes. The child keeps the human's real home, where
/// the credentials and the rollouts already live.
pub(crate) fn mcp_launch(
    config: &crate::mcp_broker::McpLaunchConfig,
    _runtime_dir: &Path,
) -> Result<crate::mcp_broker::McpProviderConfig, WireError> {
    Ok(crate::mcp_broker::McpProviderConfig {
        env_additions: vec![(
            crate::mcp_broker::MCP_TOKEN_ENV.to_string(),
            config.bearer().to_string(),
        )],
        arg_additions: vec![
            "-c".to_string(),
            format!(
                "mcp_servers.{}.url=\"{}\"",
                crate::mcp_broker::MCP_SERVER_NAME,
                config.url
            ),
            "-c".to_string(),
            format!(
                "mcp_servers.{}.bearer_token_env_var=\"{}\"",
                crate::mcp_broker::MCP_SERVER_NAME,
                crate::mcp_broker::MCP_TOKEN_ENV
            ),
        ],
        owned_paths: Vec::new(),
        owned_dirs: Vec::new(),
    })
}

/// Spawn Codex (S6 wiring, S9 live): `mcp` is the broker's launch config when
/// the session was registered for MCP tools, `None` otherwise. `Some` joins the
/// broker's server onto the launch line (`mcp_launch`) and the bearer onto the
/// child env; `None` leaves both untouched. The child keeps the human's real
/// Codex home: the credentials live there and the rollout this family resumes
/// is written there, which is also why no per-session home is minted.
pub(super) fn spawn_process(
    state: &Arc<ServerState>,
    command: PtyCommand,
    mcp: Option<crate::mcp_broker::McpLaunchConfig>,
    delivery: ProfileDelivery,
) -> Result<SpawnedSession, WireError> {
    validate_delivery(&delivery)?;
    let goals = session_goals(&command);
    let commands = session_commands(&goals, &command.cwd, &command.env);
    spawn_codex(
        state,
        command,
        mcp,
        delivery,
        ThreadRoad::Fresh,
        goals,
        commands,
    )
}

/// A resumed child: the same spawn and handshake, with `thread/resume` in
/// place of `thread/start` and nothing else changed. The app-server loads the
/// thread from disk by the `threadId` the dead generation persisted — the
/// session row's own handle — and loads nothing from our journal: no history
/// is re-sent, the provider's rollout is the conversation.
pub(super) fn spawn_process_resuming(
    state: &Arc<ServerState>,
    command: PtyCommand,
    peer_session_id: String,
    mcp: Option<crate::mcp_broker::McpLaunchConfig>,
) -> Result<SpawnedSession, WireError> {
    let goals = session_goals(&command);
    let commands = session_commands(&goals, &command.cwd, &command.env);
    spawn_codex(
        state,
        command,
        mcp,
        ProfileDelivery::none(),
        ThreadRoad::Resuming(&peer_session_id),
        goals,
        commands,
    )
}

/// The command surface of one session: the list read off disk under the
/// resolved Codex home and this session's start path. Codex publishes no list
/// over the protocol, so the filesystem is the only source (`listCommands`,
/// and the `listCodexCustomPrompts` / `listCodexSkills` walks it calls).
fn session_commands(goals: &Goals, cwd: &Path, env: &[(String, String)]) -> Arc<CodexCommands> {
    let home = env
        .iter()
        .find(|(key, _)| key == "CODEX_HOME")
        .map(|(_, value)| std::path::PathBuf::from(value));
    #[cfg(test)]
    let home = home.unwrap_or_else(|| crate::test_dirs::test_temp_dir("devboule-codex-spawn-home"));
    #[cfg(not(test))]
    let home = home.unwrap_or_else(crate::codex_command_catalog::resolve_home);
    Arc::new(CodexCommands::new(&home, Some(cwd), goals.enabled()))
}

fn session_goals(command: &PtyCommand) -> Goals {
    #[cfg(test)]
    {
        Goals::from_version_output(
            command
                .env
                .iter()
                .find(|(key, _)| key == "CODEX_VERSION")
                .map(|(_, value)| value.as_str())
                .unwrap_or("unknown"),
        )
    }
    #[cfg(not(test))]
    Goals::probe(&command.program, &command.args)
}

fn spawn_codex(
    state: &Arc<ServerState>,
    command: PtyCommand,
    mcp: Option<crate::mcp_broker::McpLaunchConfig>,
    delivery: ProfileDelivery,
    road: ThreadRoad<'_>,
    goals: Goals,
    commands: Arc<CodexCommands>,
) -> Result<SpawnedSession, WireError> {
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
    let carrier_args: Vec<String> = carrier
        .as_ref()
        .map(|carrier| carrier.arg_additions.clone())
        .unwrap_or_default();

    let mut process = Command::new(&command.program);
    process
        .args(&command.args)
        // The goals feature Codex gates by version, and only when the gate
        // passed: an older binary rejects the flag and the child would not
        // start at all (`spawnAppServer` :7087-7091).
        .args(goals.launch_args())
        .args(&carrier_args)
        .current_dir(&command.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &command.env {
        process.env(key, value);
    }
    // The carrier env, only when the broker minted one: the bearer as child
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
            WireError::new(
                ErrorCode::Io,
                format!("Could not create the Codex process job: {error}"),
            )
        })?;
        let handle = child.as_raw_handle();
        // This agent's own fresh job; why no shared job is ever an
        // assignment target is stated once, at open_pty_session.
        if let Err(error) = process_job.assign(handle) {
            terminate_process(&mut child);
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
        WireError::new(
            ErrorCode::Io,
            format!("Could not create the Codex process job: {error}"),
        )
    })?;
    #[cfg(not(windows))]
    let os_handle = None;

    let stdin = child.stdin.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "Codex did not provide stdin.")
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "Codex did not provide stdout.")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "Codex did not provide stderr.")
    })?;
    let process = Arc::new(Mutex::new(child));
    let stdin = Arc::new(Mutex::new(Some(stdin)));
    let next_id = Arc::new(AtomicU64::new(1));
    let mut stdout = CodexStdout::spawn(stdout).map_err(|error| {
        terminate_shared_process(&process);
        WireError::new(
            ErrorCode::Io,
            format!("Could not read Codex stdout: {error}"),
        )
    })?;
    let handshake = perform_handshake(&mut stdout, &stdin, &next_id, &command.cwd, &mode_id, road)
        .inspect_err(|_| {
            terminate_shared_process(&process);
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
        return Err(error);
    }
    // The fast-mode tick, seeded after the model so the gate is read against
    // the model the child actually runs, and before any turn can be sent.
    if let Err(error) = seed_fast_mode(&state, &delivery) {
        terminate_shared_process(&process);
        return Err(error);
    }
    let peer_session_id = state.thread_id();
    // One registration table for the requests this client awaits answers to
    // (A2-03), shared by the steerer that registers and the reader that
    // delivers.
    let requests = Arc::new(CodexRequests::new());
    let out_of_band: Option<Arc<dyn OutOfBandCommands>> = Some(Arc::new(CodexOutOfBand::new(
        Arc::clone(&stdin),
        Arc::clone(&next_id),
        Arc::clone(&state),
        Arc::clone(&commands),
    )));
    // S8 trigger bundle, cloned handles only (never the reader — see
    // `CodexVerifyBundle`): present exactly when a carrier was installed, so
    // the `None` road — today's only road — verifies nothing and changes nothing.
    let pending_codex_verify = codex_verify_bundle_for(&carrier, &stdin, &next_id, &requests);
    let switcher = CodexSwitcher {
        stdin: Arc::clone(&stdin),
        next_id: Arc::clone(&next_id),
        state: Arc::clone(&state),
        requests: Arc::clone(&requests),
        commands: Arc::clone(&commands),
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
        commands: Arc::clone(&commands),
        pending: Vec::new(),
    };
    // The static prompt route carries attachments, and a prompt that carries
    // one is never a command (`resolveSlashCommandInvocation` takes a string
    // prompt only, :4009) — so this route needs no command table and sends the
    // text the human typed.
    //
    // It sends Codex's own `turn/start`: it shares the
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
    };
    let reader = CodexReader {
        buffer: Vec::new(),
        discarding_oversized_line: false,
        deferred: handshake.deferred,
        manifest: Some(state.manifest()),
        available_commands: Some(SessionEvent::AvailableCommands {
            commands: commands.views(),
        }),
        commands,
        state,
        view: crate::codex_view::CodexView::new(Some(command.cwd)),
        permission_broker: Arc::clone(&permission_broker),
        response_ids,
        stdin: Arc::clone(&stdin),
        next_id,
        requests,
        compactions: crate::codex_compaction::CodexCompactions::default(),
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
        out_of_band,
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
    commands: Arc<CodexCommands>,
}

struct CodexSteerer {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    state: Arc<CodexState>,
    requests: Arc<CodexRequests>,
    commands: Arc<CodexCommands>,
}

impl SessionSteerer for CodexSteerer {
    fn steer_active_turn(
        &mut self,
        text: &str,
        turn: &mut TurnToken<'_>,
    ) -> Result<bool, WireError> {
        // Paseo refuses listed slash commands as steers so the caller replaces
        // the turn and `buildCommandPromptInput` can expand them (:4326).
        if self.commands.is_picked_command(text) {
            return Ok(false);
        }
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
            commands: Arc::clone(&self.commands),
        })
    }
}

/// One command's line as the session sees it.
///
/// Paseo emits these as an assistant message (:4985-5008); the daemon's own
/// words about its own request are a notice, never a message attributed to the
/// model, which is the channel `SessionNotice` exists for.
fn command_notice(text: String, failed: bool) -> SessionEvent {
    SessionEvent::SessionNotice {
        text,
        severity: if failed {
            NoticeSeverity::Warning
        } else {
            NoticeSeverity::Info
        },
    }
}

pub(super) struct CodexOutOfBand {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    state: Arc<CodexState>,
    commands: Arc<CodexCommands>,
}

impl CodexOutOfBand {
    pub(super) fn new(
        stdin: Arc<Mutex<Option<ChildStdin>>>,
        next_id: Arc<AtomicU64>,
        state: Arc<CodexState>,
        commands: Arc<CodexCommands>,
    ) -> Self {
        Self {
            stdin,
            next_id,
            state,
            commands,
        }
    }
}

impl OutOfBandCommands for CodexOutOfBand {
    fn handles_out_of_band(&self, text: &str) -> bool {
        self.commands.command(text).is_some()
    }

    fn skips_first_prompt_composition(&self, text: &str) -> bool {
        self.commands.is_picked_command(text)
    }

    fn run_out_of_band(&self, text: &str, runtime: &Arc<SessionRuntime>) {
        let Some(command) = self.commands.command(text) else {
            return;
        };
        let Some((method, params)) = command.request(&self.state.thread_id()) else {
            if let Some(line) = command.outcome(None) {
                let _ = runtime.publish_daemon_event(command_notice(line, false));
            }
            return;
        };
        let id = format!("d-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        if !self.commands.owe(&id, &command) {
            let _ = runtime.publish_daemon_event(command_notice(
                "Could not track the Codex command response; retry the command.".to_string(),
                true,
            ));
            return;
        }
        if let Err(error) = send_frame(&self.stdin, &request_frame(&id, method, params), "Codex") {
            self.commands.forget(&id);
            let line = command
                .outcome(Some(&error.message))
                .unwrap_or_else(|| format!("Codex could not run the command: {}", error.message));
            let _ = runtime.publish_daemon_event(command_notice(line, true));
        }
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
            commands: Arc::clone(&self.commands),
        })
    }

    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(CodexSteerer {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            state: Arc::clone(&self.state),
            requests: Arc::clone(&self.requests),
            commands: Arc::clone(&self.commands),
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
    commands: Arc<CodexCommands>,
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
        // A prompt that names one of this session's commands goes as the form
        // that command has — an expanded custom prompt, or skill and text
        // blocks for a skill (`buildCommandPromptInput` :4028-4056). Everything else, and
        // every `/compact` and `/goal`, keeps the text the human typed: those
        // two are answered out of band before the writer is reached, and a
        // prompt that carries an image is not a command at all.
        let input = self
            .commands
            .prompt_input_checked(&text)
            .map_err(io::Error::other)?;
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
            {
                let mut params = turn_start_params(
                    &self.state.thread_id(),
                    &text,
                    policy_mode.as_deref(),
                    Some(&model),
                    effort.as_deref(),
                    self.state.service_tier().as_deref(),
                );
                if let Some(input) = input {
                    params["input"] = input;
                }
                params
            },
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
        // Grace for a natural exit first — but the process must be reaped on
        // EVERY path below, never just the kill path: the old early `return`
        // on a reaped child skipped the reap (S9 road test caught it: a child
        // that exits on stdin close left its state behind).
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
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self {
            process: Arc::clone(&self.process),
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            state: Arc::clone(&self.state),
            permission_broker: Arc::clone(&self.permission_broker),
            cancelled: Arc::clone(&self.cancelled),
        })
    }
}

struct Handshake {
    thread_id: String,
    catalog: CodexCatalog,
    deferred: Vec<Value>,
}

/// The thread road the handshake takes: a fresh `thread/start`, or the
/// `thread/resume` that loads the persisted thread by the handle a dead
/// generation stored.
#[derive(Clone, Copy)]
enum ThreadRoad<'a> {
    Fresh,
    Resuming(&'a str),
}

fn perform_handshake(
    stdout: &mut CodexStdout,
    stdin: &Mutex<Option<ChildStdin>>,
    next_id: &AtomicU64,
    cwd: &Path,
    mode_id: &str,
    road: ThreadRoad<'_>,
) -> Result<Handshake, WireError> {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut deferred = Vec::new();
    let _initialize = request_response(
        stdout,
        stdin,
        next_id,
        "initialize",
        initialize_params(),
        deadline,
        &mut deferred,
    )?;
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
    let (method, params) = match road {
        ThreadRoad::Fresh => ("thread/start", thread_start_params(cwd, mode_id)),
        ThreadRoad::Resuming(thread_id) => ("thread/resume", thread_resume_params(thread_id)),
    };
    let thread_response = request_response(
        stdout,
        stdin,
        next_id,
        method,
        params,
        deadline,
        &mut deferred,
    )?;
    let thread_id = thread_response
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| handshake_error(&format!("{method} response had no thread.id")))?
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
    service_tier: Option<&str>,
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
    insert_service_tier(&mut params, service_tier);
    Value::Object(params)
}

/// The profile's fast-mode tick, on the frame Codex reads its service tier
/// from — Paseo's `params.serviceTier = "fast"`, sent on every turn because
/// the server keeps no tier between them. `None` writes no key: a profile that
/// never ticked fast mode sends no parameter rather than one spelling "off"
/// the provider has no meaning for.
fn insert_service_tier(params: &mut serde_json::Map<String, Value>, service_tier: Option<&str>) {
    if let Some(service_tier) = service_tier {
        params.insert(
            "serviceTier".to_string(),
            Value::String(service_tier.to_string()),
        );
    }
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
    service_tier: Option<&str>,
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
    insert_service_tier(&mut params, service_tier);
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
    let service_tier = state.service_tier();
    turn_start_params_with_images(
        &state.thread_id(),
        text,
        image_paths,
        policy_mode.as_deref(),
        Some(&model),
        effort.as_deref(),
        service_tier.as_deref(),
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
/// the feature would fail silent. `pub(super)` for the provider trait's
/// `image_delivery` delegation (`provider.rs`) — the fact stays in the
/// family module.
pub(super) fn codex_delivery() -> super::ImageDelivery {
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

/// `thread/resume` takes the handle alone: the thread loaded from disk
/// already carries its cwd, model and policy (measured against the installed
/// app-server schema: `ThreadResumeParams` requires only `threadId`).
fn thread_resume_params(thread_id: &str) -> Value {
    serde_json::json!({ "threadId": thread_id })
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
    /// The command list is journaled at session start so reattachments recover it.
    available_commands: Option<SessionEvent>,
    commands: Arc<CodexCommands>,
    state: Arc<CodexState>,
    view: crate::codex_view::CodexView,
    permission_broker: Arc<PermissionBroker>,
    response_ids: Arc<Mutex<HashMap<u64, Value>>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    requests: Arc<CodexRequests>,
    compactions: crate::codex_compaction::CodexCompactions,
}

impl CodexReader {
    fn publish(&self, runtime: &SessionRuntime, event: SessionEvent, seq: Option<u64>) {
        let _ = runtime.publish_agent_event_with_seq(event, None, seq);
    }

    fn dispatch_value(&mut self, value: Value, runtime: &Arc<SessionRuntime>) {
        let event_seq = runtime.journal_agent_envelope(&value);
        // Answered here rather than in the view: one of the two compaction
        // channels is keyed by the thread this session owns, and the pair has
        // to be counted against each other.
        if let Some(event) = self.compactions.event(&value, &self.state.thread_id()) {
            let _ = runtime.publish_daemon_event(event);
            return;
        }
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
            } else if method == "turn/completed" {
                for event in self.compactions.turn_ended() {
                    let _ = runtime.publish_daemon_event(event);
                }
            } else if let Some(id) = value.get("id") {
                let _ = send_frame(&self.stdin, &method_not_supported_frame(id), "Codex");
            }
        } else if let Some(id) = value.get("id") {
            // A response to one of this client's own requests: hand it to the
            // waiter that registered that id before anything else looks at it
            // (A2-03). A response whose id matches no waiter is ignored.
            self.requests.deliver(&value);
            if let Some(turn_id) = turn_id_from_response(&value) {
                self.state.set_turn(Some(turn_id));
            }
            let error = value.get("error").map(|error| {
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex returned an error without a message")
                    .to_string()
            });
            // A command's answer is told in the command's own words, Paseo's
            // including (``executeCompactCommand`` :5027-5031,
            // ``executeGoalSubcommand`` :5081-5085); any other response keeps
            // the pre-existing notice of Codex's message. The branch is
            // exclusive so one failed request is not reported twice.
            match self.commands.answer(id, error.as_deref()) {
                Answer::Ours(text) => {
                    if let Some(text) = text {
                        let _ = runtime.publish_daemon_event(command_notice(text, error.is_some()));
                    }
                }
                Answer::NotOurs => {
                    if let Some(error) = error.as_deref() {
                        let _ = runtime
                            .publish_session_notice(error.to_string(), NoticeSeverity::Warning);
                    }
                }
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
            if !matches!(error, PermissionResponseError::AlreadyRecorded) {
                // The repeat-refusal's one plain notice is already up; only
                // the cancel decision goes back for that case.
                let _ = runtime.publish_session_notice(
                    format!("Could not queue Codex permission request: {error}"),
                    NoticeSeverity::Warning,
                );
            }
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
            // The command list must be an AgentReport row: replay has no
            // Codex envelope from which to reconstruct a filesystem-only list.
            if let Some(commands) = self.available_commands.take() {
                let _ = runtime.publish_daemon_event(commands);
            }
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
/// The command surface of a session with nothing on disk: an empty Codex home,
/// no workspace, no goals. A test that exercises the transport still has to
/// hand the client a surface, and this keeps it off the real `~/.codex`.
pub(crate) fn empty_commands() -> Arc<CodexCommands> {
    let home = crate::test_dirs::test_temp_dir("devboule-codex-empty-home");
    let commands = Arc::new(CodexCommands::new(&home, None, false));
    let _ = std::fs::remove_dir_all(&home);
    commands
}

#[cfg(test)]
#[path = "codex_client_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "codex_client_delivery_tests.rs"]
mod delivery_tests;

/// The fake Codex child and the temp home the two files below share.
#[cfg(test)]
#[path = "codex_command_test_support.rs"]
mod command_test_support;

/// The requests the commands write, the form a picked prompt or skill takes,
/// and the launch line the version gate decides.
#[cfg(test)]
#[path = "codex_client_command_tests.rs"]
mod command_tests;

/// The events a command answer or a compaction notification turns into.
#[cfg(test)]
#[path = "codex_client_compaction_tests.rs"]
mod compaction_tests;
