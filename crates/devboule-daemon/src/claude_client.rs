//! Claude stream-json stdio adapter for live agent sessions.
//!
//! This module owns only the process and protocol adapters. The parent
//! session module still owns the runtime, attachment queue, journal,
//! liveness monitor, registry and teardown order. Permissions go through
//! the existing [`super::permission_broker::PermissionBroker`]; this file does not
//! fork it.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use devboule_protocol::{ErrorCode, PermissionOption, SessionEvent, SessionModel, WireError};
use serde_json::Value;

use super::permission_broker::{PermissionBroker, PermissionResponseError, PermissionSender};
use super::PtyCommand;
use super::{
    write_child_stdin, ModelSwitcher, ReaderDispatch, SessionKiller, SessionRuntime,
    SessionSteerer, SpawnedSession, StderrSource, StdioWaitableChild, TurnToken,
};
use crate::attachment_store::AttachmentStore;
use crate::claude_view::ClaudeView;
use crate::mcp_broker::McpLaunchConfig;
use crate::paths::RuntimePaths;
use crate::process_tree::{JobObject, ProcessHandle};
use crate::profile_delivery::{DeliveredFeature, ProfileDelivery};
use crate::server::ServerState;

const COMMAND_ENV: &str = "DEVBOULE_CLAUDE_COMMAND";
const MAX_LINE_BYTES: usize = 10 * 1024 * 1024;
const CONTROL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

type ClaudeModeResponses = Arc<Mutex<HashMap<String, Sender<Result<(), String>>>>>;

/// The delivery's effort requests that have been written and are waiting for
/// the CLI's answer: request id → the effort the profile named. The session
/// reader answers them in `ClaudeReader::dispatch_control_response` — a
/// refused effort fails the session the way a refused initial mode does,
/// instead of disappearing while the child runs at the CLI's own level (the
/// R2a audit's F2).
type ClaudeDeliverySettings = Arc<Mutex<HashMap<String, String>>>;

struct ClaudePendingControl {
    request_id: String,
    input: Value,
}

enum ClaudeModeGateState {
    AwaitingResponse {
        request_id: String,
        requested_mode: String,
    },
    Ready,
    Failed(String),
}

struct ClaudeModeGate {
    state: ClaudeModeGateState,
    pending_frames: Vec<Vec<u8>>,
}

type ClaudeModeGateRef = Arc<Mutex<ClaudeModeGate>>;

/// Launch-time mode gate wiring: the stdin the gate writes through, the gate
/// itself, the response deadline, and the delivery's effort requests the
/// reader must answer.
struct ClaudeModeGateWiring {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    gate: ClaudeModeGateRef,
    timeout: Duration,
    delivery_settings: ClaudeDeliverySettings,
}

impl ClaudeModeGateWiring {
    fn new(stdin: Arc<Mutex<Option<ChildStdin>>>, gate: ClaudeModeGateRef) -> Self {
        Self {
            stdin,
            gate,
            timeout: CONTROL_RESPONSE_TIMEOUT,
            delivery_settings: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// Resolve `claude` plus the measured stream-json launch args. Honors
/// `DEVBOULE_CLAUDE_COMMAND` as a JSON string array, matching the ACP override.
pub(super) fn resolve_command(_paths: &RuntimePaths) -> Result<PtyCommand, WireError> {
    let cwd = std::env::current_dir().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not determine agent working directory: {error}"),
        )
    })?;
    let (mut argv, spawn_path_env): (Vec<String>, Option<(String, String)>) = match std::env::var(
        COMMAND_ENV,
    ) {
        Ok(argv) => (
            serde_json::from_str(&argv).map_err(|error| {
                WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("{COMMAND_ENV} must be a non-empty JSON string array: {error}"),
                )
            })?,
            None,
        ),
        Err(_) => {
            let Some(agent) = crate::provider_catalog::find_available("claude") else {
                return Err(WireError::new(
                    ErrorCode::Io,
                    format!(
                        "Claude was not found on PATH. Set {COMMAND_ENV} to a non-empty JSON string array to choose a command explicitly."
                    ),
                ));
            };
            let Some(stream_json) = agent.stream_json_command else {
                return Err(WireError::new(
                    ErrorCode::Io,
                    "Claude is installed but has no stream-json launch args.",
                ));
            };
            (stream_json, agent.spawn_path_env)
        }
    };
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("{COMMAND_ENV} must contain an executable."),
        ));
    }
    let program = argv.remove(0);
    Ok(
        PtyCommand::new(program, argv, cwd, spawn_path_env.into_iter().collect())
            .with_provider_id("claude"),
    )
}

fn launch_in_bypass_mode(argv: Vec<String>) -> Vec<String> {
    let mut args = strip_flag(argv, "--permission-mode");
    args.extend([
        "--permission-mode".to_string(),
        "bypassPermissions".to_string(),
    ]);
    args
}

/// The CLI runs its own default model when no `--model` is passed, so the tab
/// would name a model the process is not running. Pin the catalog's current
/// model at launch; a later set_model control request still overrides it.
fn launch_with_model(argv: Vec<String>, model_id: Option<&str>) -> Vec<String> {
    let mut args = strip_flag(argv, "--model");
    if let Some(model_id) = model_id {
        args.extend(["--model".to_string(), model_id.to_string()]);
    }
    args
}

fn strip_flag(argv: Vec<String>, flag: &str) -> Vec<String> {
    let with_equals = format!("{flag}=");
    let mut args = Vec::with_capacity(argv.len());
    let mut skip_value = false;
    for arg in argv {
        if skip_value {
            skip_value = false;
            continue;
        }
        if arg == flag {
            skip_value = true;
            continue;
        }
        if arg.starts_with(&with_equals) {
            continue;
        }
        args.push(arg);
    }
    args
}

/// The model the CLI runs when no `--model` is passed is a *preference* of the
/// catalog; the profile's model, when one is delivered, is not. Two pure
/// refusal checks sit below so the tests can hold the sentences without a
/// [`ServerState`]: **absent vocabulary** and **unknown id** are different
/// refusals, and collapsing them sends a human hunting a typo when the
/// provider simply has no dial.
///
/// Model — refuse, never substitute: cost and capability are the premise of
/// the human's choice.
fn validate_model_choice(models: &[SessionModel], model_id: Option<&str>) -> Result<(), WireError> {
    let Some(model_id) = model_id else {
        return Ok(());
    };
    if models.is_empty() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "the profile names a model, but this Claude publishes no models the daemon can deliver; the creation is refused rather than started on a different model",
        ));
    }
    // The runtime paths judge with the suffix-tolerant comparison, and the
    // CLI's own `current_model_id` — a real, displayed id — can carry the
    // `[1m]` spelling. A profile built from such an id must not be refused
    // here when the live switch accepts it.
    if !models
        .iter()
        .any(|model| crate::claude_catalog::model_ids_match(&model.model_id, model_id))
    {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!(
                "Claude model '{model_id}' is not among the models this Claude publishes; the creation is refused rather than started on a different model"
            ),
        ));
    }
    Ok(())
}

/// Thinking — refuse, by symmetry with the model: the card named the value, so
/// printing it and not delivering it is the delivery defect in a smaller font.
/// A model with no thinking options at all is refused with the **absence**
/// sentence; an option outside the model's own list with the **mismatch**
/// sentence. They are not the same refusal.
fn validate_thinking_choice(
    models: &[SessionModel],
    model_id: &str,
    thinking: &str,
) -> Result<(), WireError> {
    let Some(model) = models
        .iter()
        .find(|model| crate::claude_catalog::model_ids_match(&model.model_id, model_id))
    else {
        // Unreachable through the creation path: the model choice is validated
        // first, and a profile always names a model.
        return Ok(());
    };
    let has_efforts = model
        .efforts
        .as_ref()
        .is_some_and(|efforts| !efforts.is_empty());
    if !has_efforts {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("Claude model '{model_id}' has no thinking options; the profile names one, so the creation is refused"),
        ));
    }
    if !model
        .efforts
        .as_ref()
        .expect("checked above")
        .iter()
        .any(|effort| effort.id == thinking)
    {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("Claude thinking option '{thinking}' is not among model '{model_id}'s thinking options"),
        ));
    }
    Ok(())
}

/// The model the launch pins: the delivered one, or the catalog's preference
/// for a create that resolved no profile. One function, so the argv and its
/// test cannot disagree about who wins.
fn launch_model_id(delivery: &ProfileDelivery, models: &[SessionModel]) -> Option<String> {
    delivery
        .model_id
        .clone()
        .or_else(|| crate::claude_catalog::default_model_id(models))
}

/// The creation-time refusals for one Claude delivery. This is the client
/// that owns the `--model` and `--permission-mode` flags and the effort
/// control frame, so this is where the delivery is judged: everything that
/// cannot be delivered is refused here, before a process exists, and a child
/// that exists was delivered everything its card printed.
/// The tick half of [`validate_delivery`] as one predicate, shared with the
/// tests that cross it against the pre-card gate (the re-audit's P1): the
/// gate's `Contradicts` for Claude must name exactly the pairs this refuses.
pub(crate) fn tick_contradicts(delivery: &ProfileDelivery) -> bool {
    let mode_id = delivery
        .mode_id
        .as_deref()
        .unwrap_or(crate::claude_view::DEFAULT_MODE);
    delivery.auto_accept && !crate::provider_catalog::mode_is_auto_answered(mode_id)
}

pub(super) fn validate_delivery(
    catalog: &crate::claude_catalog::ClaudeCatalogSnapshot,
    delivery: &ProfileDelivery,
) -> Result<(), WireError> {
    // Every stored feature this family has no frame for is refused here, before
    // a process exists: the launch applies `--model`, `--permission-mode` and
    // the flag-settings frame below and nothing else, so a key the Claude table
    // does not carry could only ever be a card promise that went undelivered.
    crate::profile_delivery::refuse_undeclared(
        &crate::provider_features::claude_declarations(),
        delivery.model_id.as_deref(),
        &delivery.features,
    )?;
    let mode_id = delivery
        .mode_id
        .as_deref()
        .unwrap_or(crate::claude_view::DEFAULT_MODE);
    if !crate::claude_view::mode_state(mode_id)
        .available_modes
        .iter()
        .any(|mode| mode.id == mode_id)
    {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("Claude session mode '{mode_id}' is not available."),
        ));
    }
    // `autoAccept` is a constraint on which mode is delivered: the child must
    // start in a mode the daemon's own broker answers. `bypassPermissions` is
    // that mode for Claude; the launch flag is the mechanism.
    if tick_contradicts(delivery) {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!(
                "the profile asks Claude to approve its own permission prompts and also to start in mode '{mode_id}', which asks the human; the two contradict, so the creation is refused"
            ),
        ));
    }
    // A **provisional** catalog is no vocabulary, and this codebase already
    // says so one function over: `validate_claude_effort` refuses to judge
    // when the runtime catalog is provisional. The creation path obeys the
    // same rule (the R2a audit's F4): a CLI upgrade invalidates the
    // version-keyed cache, and until the derivation finishes the fallback's
    // three aliases are a placeholder, not a list to judge a profile's saved
    // id against. Judging it there manufactured intermittent refusals of
    // legitimate profiles. The `--model` argv below takes whatever the CLI
    // knows, so nothing is delivered that this skip cannot account for; the
    // runtime effort switch judges the thinking axis against the live
    // catalog once it exists.
    if catalog.state == crate::claude_catalog::ClaudeCatalogState::Provisional {
        return Ok(());
    }
    let models = &catalog.models;
    validate_model_choice(models, delivery.model_id.as_deref())?;
    if let Some(thinking) = delivery.thinking_option_id.as_deref() {
        // A profile always names a model, so the delivered model is the one
        // whose thinking options judge the profile's choice.
        let model_id = delivery
            .model_id
            .clone()
            .or_else(|| crate::claude_catalog::default_model_id(models));
        if let Some(model_id) = model_id {
            validate_thinking_choice(models, &model_id, thinking)?;
        }
    }
    Ok(())
}

/// The `~/.claude/projects/<slug>` directory for one cwd, as the CLI lays
/// it out: every byte outside `[A-Za-z0-9-]` becomes `-`. Measured against
/// the directories on this machine (`C:\Users\gualt\Desktop\New
/// devboule\devboule-v2` sits under
/// `C--Users-gualt-Desktop-New-devboule-devboule-v2`); a cwd with characters
/// outside the observed set keeps the same rule, and a mismatch fails closed
/// in the history lookup below, never as a wrong file.
fn claude_projects_slug(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|cell| {
            if cell.is_ascii_alphanumeric() || cell == '-' {
                cell
            } else {
                '-'
            }
        })
        .collect()
}

/// Where the CLI keeps its own conversations.
fn claude_home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Closed alphabet for a provider session id, not a blocklist: anything
/// outside `[A-Za-z0-9-]` refuses. `Path::join` discards the whole base when
/// the pushed segment carries a Windows prefix (`C:evil`, `C:\evil`, UNC),
/// so rejecting `/`, `\` and `..` is not enough — and the id the journal
/// holds is a provider UUID, which this alphabet accepts trivially.
fn valid_peer_session_id(peer_session_id: &str) -> bool {
    !peer_session_id.is_empty()
        && peer_session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// The conversation file a `--resume` needs. The exact slug first; then one
/// stat per sibling slug directory, because the CLI resolves `--resume` by
/// id (measured: a session resumed from a cwd that is not its own prints no
/// "not found"), while our slug rule for an exotic cwd may be wrong. `None`
/// therefore means the conversation is on no disk we can see — deleted by
/// the human or rotated by the CLI — never "we looked in one place".
///
/// `home` is a parameter rather than read here so tests pin the rule without
/// moving the process environment.
/// What one lookup of the conversation file established. Only [`Absent`]
/// refuses: the naming rule or the directory itself said the file is not
/// there. [`Unreadable`] is a lookup that could not happen — a locked or
/// half-taken directory, an antivirus moment — and says NOTHING about the
/// conversation, so it must never feed the disown mark: the refusal it would
/// trigger is not reversible, and the lookup it rests on was never made.
enum ClaudeHistoryLookup {
    /// The file is there. (The lookup used to return the path; no caller
    /// ever read it — the `--resume` argv carries the id, not the path.)
    Found,
    Absent,
    Unreadable(std::io::Error),
}

fn find_claude_history(home: &Path, cwd: &Path, peer_session_id: &str) -> ClaudeHistoryLookup {
    if !valid_peer_session_id(peer_session_id) {
        // The naming rule cannot produce a file name for this id, so under
        // the rule the daemon and the CLI share, the file does not exist:
        // looked, and not there.
        return ClaudeHistoryLookup::Absent;
    }
    let projects = home.join(".claude").join("projects");
    let file = format!("{peer_session_id}.jsonl");
    let exact = projects.join(claude_projects_slug(cwd)).join(&file);
    if exact.is_file() {
        return ClaudeHistoryLookup::Found;
    }
    let siblings = match std::fs::read_dir(&projects) {
        Ok(siblings) => siblings,
        Err(error) => return ClaudeHistoryLookup::Unreadable(error),
    };
    if siblings
        .filter_map(Result::ok)
        .any(|sibling| sibling.path().join(&file).is_file())
    {
        return ClaudeHistoryLookup::Found;
    }
    ClaudeHistoryLookup::Absent
}

/// The refusal when there is nothing to resume: it names the conversation
/// and the file's expected place, not a generic spawn error. The code is
/// the resume road's **internal** disown sentinel — the daemon has
/// positively confirmed, from its own filesystem, that there is nothing to
/// resume, which is stronger evidence than any agent's answer (ACP raises
/// the same sentinel for its confirmed disowns). The resume arm rewrites it
/// to the wire's `InvalidRequest` before the caller sees it: the sentence
/// below is what the app has always read.
fn missing_history_error(peer_session_id: &str, cwd: &Path, home: &Path) -> WireError {
    let expected = home
        .join(".claude")
        .join("projects")
        .join(claude_projects_slug(cwd))
        .join(format!("{peer_session_id}.jsonl"));
    WireError::new(
        ErrorCode::SessionNotFound,
        format!(
            "Claude conversation '{peer_session_id}' has no history file (expected at {}): it was deleted or rotated, so there is nothing to resume.",
            expected.to_string_lossy()
        ),
    )
}

/// The resume half of the launch argv: `--resume <peer>` on top of the
/// measured stream-json base. No `--model` pin — the pin names the card's
/// model for a new conversation, and a resumed conversation already has one.
fn push_resume_flag(mut args: Vec<String>, peer_session_id: &str) -> Vec<String> {
    args.push("--resume".to_string());
    args.push(peer_session_id.to_string());
    args
}

pub(super) fn spawn_process(
    state: &Arc<ServerState>,
    command: PtyCommand,
    mcp: Option<McpLaunchConfig>,
    delivery: ProfileDelivery,
) -> Result<SpawnedSession, WireError> {
    validate_delivery(&state.claude_models(), &delivery)?;
    let requested_mode = delivery
        .mode_id
        .as_deref()
        .unwrap_or(crate::claude_view::DEFAULT_MODE)
        .to_string();
    let mut args = command.args.clone();
    if let Some(path) = mcp
        .as_ref()
        .and_then(|config| config.claude_config_path.as_ref())
    {
        args.push("--mcp-config".to_string());
        args.push(path.to_string_lossy().into_owned());
        if !args.iter().any(|arg| arg == "--strict-mcp-config") {
            args.push("--strict-mcp-config".to_string());
        }
    }
    // The delivered model, when a profile named one, is the launch flag — the
    // CLI's own default would run a different model at a different price than
    // the one the card named. A create that resolved no profile keeps the
    // catalog's preference, exactly as before this delivery existed.
    let args = launch_with_model(
        launch_in_bypass_mode(args),
        launch_model_id(&delivery, &state.claude_models().models).as_deref(),
    );
    spawn_claude_child(&command, args, requested_mode, &delivery, None)
}

/// A resumed child: the same launch minus the `--model` pin (the resumed
/// conversation keeps its own model) plus `--resume <peer>`, onto the same
/// session row. The provider's file must still be on disk — spawning into a
/// deleted or rotated conversation reads as a hang, so its absence refuses
/// here, naming the file, before any process exists.
pub(super) fn spawn_process_resuming(
    command: PtyCommand,
    peer_session_id: String,
    mcp: Option<McpLaunchConfig>,
) -> Result<SpawnedSession, WireError> {
    let home = claude_home_dir().ok_or_else(|| {
        WireError::new(
            ErrorCode::InvalidRequest,
            format!(
                "Claude conversation '{peer_session_id}' cannot be resumed without a home directory to look its history file up in."
            ),
        )
    })?;
    match find_claude_history(&home, &command.cwd, &peer_session_id) {
        ClaudeHistoryLookup::Found => {}
        ClaudeHistoryLookup::Absent => {
            return Err(missing_history_error(&peer_session_id, &command.cwd, &home))
        }
        ClaudeHistoryLookup::Unreadable(error) => {
            // A lookup that could not happen concludes nothing. It is an
            // honest failure — not the disown sentinel — so the offer stands
            // and the human can simply try again when the directory lets go.
            return Err(WireError::new(
                ErrorCode::Io,
                format!(
                    "Could not look up the Claude conversation '{peer_session_id}' under {}: {error}; the resume was not attempted, so nothing was concluded about the conversation.",
                    home.join(".claude").join("projects").to_string_lossy()
                ),
            ));
        }
    }
    let delivery = ProfileDelivery::none();
    let requested_mode = crate::claude_view::DEFAULT_MODE.to_string();
    let mut args = command.args.clone();
    if let Some(path) = mcp
        .as_ref()
        .and_then(|config| config.claude_config_path.as_ref())
    {
        args.push("--mcp-config".to_string());
        args.push(path.to_string_lossy().into_owned());
        if !args.iter().any(|arg| arg == "--strict-mcp-config") {
            args.push("--strict-mcp-config".to_string());
        }
    }
    let args = push_resume_flag(launch_in_bypass_mode(args), &peer_session_id);
    spawn_claude_child(
        &command,
        args,
        requested_mode,
        &delivery,
        Some(peer_session_id),
    )
}

/// The child both roads share: process, job containment, stdio, the initial
/// mode gate, the broker, the reader. Fresh and resumed differ only in the
/// argv they arrive with and the peer id they report.
fn spawn_claude_child(
    command: &PtyCommand,
    args: Vec<String>,
    requested_mode: String,
    delivery: &ProfileDelivery,
    resume_peer: Option<String>,
) -> Result<SpawnedSession, WireError> {
    let mut process = Command::new(&command.program);
    process
        .args(&args)
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
        process.creation_flags(0x0800_0000);
    }
    let mut child = process.spawn().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not start Claude {}: {error}", command.program),
        )
    })?;

    #[cfg(windows)]
    let (process_job, os_handle) = {
        use std::os::windows::io::AsRawHandle;
        let process_job = JobObject::new().map_err(|error| {
            terminate_process(&mut child);
            WireError::new(
                ErrorCode::Io,
                format!("Could not create the Claude process job: {error}"),
            )
        })?;
        let handle = child.as_raw_handle();
        // This agent's own fresh job; why no shared job is ever an
        // assignment target is stated once, at open_pty_session.
        if let Err(error) = process_job.assign(handle) {
            terminate_process(&mut child);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not contain the Claude process: {error}"),
            ));
        }
        let os_handle = match ProcessHandle::duplicate(handle) {
            Ok(duplicated) => Some(duplicated),
            Err(error) => {
                eprintln!("could not duplicate Claude process handle for OS liveness: {error}");
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
            format!("Could not create the Claude process job: {error}"),
        )
    })?;
    #[cfg(not(windows))]
    let os_handle = None;

    let stdin = child.stdin.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "Claude did not provide stdin.")
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "Claude did not provide stdout.")
    })?;
    // Wrapped here rather than at the struct below: the fast-mode confirmation
    // waits on this pipe before any session reader exists, exactly as the ACP
    // creation-time switch does for the same reason.
    let mut stdout = std::io::BufReader::new(stdout);
    let stderr = child.stderr.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "Claude did not provide stderr.")
    })?;

    let process = Arc::new(Mutex::new(child));
    let stdin = Arc::new(Mutex::new(Some(stdin)));
    let controls = Arc::new(Mutex::new(HashMap::new()));
    let mode_responses = Arc::new(Mutex::new(HashMap::new()));
    let next_id = Arc::new(AtomicU64::new(1));
    let mode_gate = match start_initial_mode(&stdin, &next_id, requested_mode.clone()) {
        Ok(mode_gate) => mode_gate,
        Err(error) => {
            if let Ok(mut process) = process.lock() {
                terminate_process(&mut process);
            }
            drop(process_job);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not send initial Claude mode request: {error}"),
            ));
        }
    };
    // The thinking option the profile named, delivered synchronously right
    // after the mode frame it follows (both are in the pipe before a reader
    // exists, so the gate — opened only by the session reader's delivery of
    // the mode response — cannot flush a prompt between or before them), and
    // with its response tracked: a CLI that refuses the effort fails the
    // session instead of silently running at its own level (the R2a audit's
    // F2). A write failure here is a refused creation, like a failed mode
    // write above.
    let delivery_settings: ClaudeDeliverySettings = Arc::new(Mutex::new(HashMap::new()));
    if let Some(thinking) = delivery.thinking_option_id.as_deref() {
        if let Err(error) = send_initial_effort(&stdin, &next_id, &delivery_settings, thinking) {
            if let Ok(mut process) = process.lock() {
                terminate_process(&mut process);
            }
            drop(process_job);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not send Claude effort request: {error}"),
            ));
        }
    }
    // The fast-mode flag the profile ticked, on the same control frame and
    // with the same tracked answer as the effort above: a stored `false` is
    // the CLI's own default and asks for nothing, and a CLI that will not take
    // `true` fails the session rather than leaving a child running unflagged
    // behind a card that said it was fast.
    let fast_mode = delivery
        .features
        .iter()
        .find(|feature| feature.id() == crate::provider_features::FAST_MODE_FEATURE)
        .map(|feature| matches!(feature, DeliveredFeature::Toggle { on: true, .. }));
    // The fast-mode flag the profile ticked, on the same control frame and with
    // the same tracked answer as the effort above. A stored `false` is the CLI's
    // own default and asks for nothing.
    //
    // Unlike the effort, this one is **awaited before the child is accepted**.
    // The rule is the delivery's own — a child that exists was delivered
    // everything its card printed — and the effort's async answer does not
    // satisfy it: a CLI that consumes the frame and never replies, or answers
    // with an error after the spawn has returned, leaves a running session whose
    // transcript carries a session error instead of the creation the human
    // approved. Paseo can afford the fire-and-forget shape because it also puts
    // `fastMode` in the child's own launch settings
    // (`buildSettingsOptions` → `settings: { fastMode }`), so the value is part
    // of how the process starts and its SDK awaits the control call before the
    // first turn. This family sends no launch setting it has not measured, so
    // the awaited frame is what makes the promise true.
    let mut prelude = Vec::new();
    if fast_mode == Some(true) {
        let request_id = match send_initial_fast_mode(&stdin, &next_id, &delivery_settings, true) {
            Ok(request_id) => request_id,
            Err(error) => {
                if let Ok(mut process) = process.lock() {
                    terminate_process(&mut process);
                }
                drop(process_job);
                return Err(WireError::new(
                    ErrorCode::Io,
                    format!("Could not send Claude fast-mode request: {error}"),
                ));
            }
        };
        let outcome = confirm_delivery_settings(
            &mut stdout,
            &mut prelude,
            &request_id,
            &delivery_settings,
            CONTROL_RESPONSE_TIMEOUT,
        );
        if let Err(error) = outcome {
            if let Ok(mut process) = process.lock() {
                terminate_process(&mut process);
            }
            drop(process_job);
            return Err(error);
        }
    }
    let sender = claude_permission_sender(Arc::clone(&stdin), Arc::clone(&controls));
    let permission_broker = PermissionBroker::with_sender(sender);
    let stderr_source = match ClaudeStderr::start(stderr) {
        Ok(source) => source,
        Err(error) => {
            if let Ok(mut process) = process.lock() {
                terminate_process(&mut process);
            }
            drop(process_job);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not drain Claude stderr: {error}"),
            ));
        }
    };
    let writer = ClaudeWriter {
        stdin: Arc::clone(&stdin),
        pending: Vec::new(),
        mode_gate: Some(Arc::clone(&mode_gate)),
    };
    // The static prompt route needs the same stdin and the same gate: an image
    // frame must queue behind the initial mode response exactly as a text
    // frame does.
    let static_prompt = Arc::new(ClaudeStaticPrompt::new(
        Arc::clone(&stdin),
        Some(Arc::clone(&mode_gate)),
    ));
    let killer = ClaudeKiller {
        process: Arc::clone(&process),
        stdin: Arc::clone(&stdin),
        next_id: Arc::clone(&next_id),
        permission_broker: Arc::clone(&permission_broker),
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    let mut wiring = ClaudeModeGateWiring::new(Arc::clone(&stdin), Arc::clone(&mode_gate));
    wiring.delivery_settings = Arc::clone(&delivery_settings);
    let mut reader_dispatch = ClaudeReader::with_mode_gate(
        ClaudeView::new(Some(command.cwd.clone())),
        Arc::clone(&permission_broker),
        Arc::clone(&controls),
        Arc::clone(&mode_responses),
        Arc::clone(&next_id),
        wiring,
    );
    // The fast-mode wait read ahead in this same pipe, and whatever it saw that
    // was not its own answer — the init event, the mode response, an MCP status
    // line — is seeded here so the session reader parses it once, through the
    // parser that understands all of it. A second reader of the same bytes is
    // where a delivery confirmation and the mode gate would disagree about which
    // lines had already been handled. Empty unless a fast-mode frame was sent.
    reader_dispatch.buffer = prelude;
    Ok(SpawnedSession {
        process_job,
        master: None,
        killer: Box::new(killer),
        switcher: Some(Box::new(ClaudeSwitcher {
            stdin: Arc::clone(&stdin),
            next_id: Arc::clone(&next_id),
            mode_responses,
            mode_gate: Some(Arc::clone(&mode_gate)),
        })),
        child: Box::new(StdioWaitableChild { process }),
        writer: Arc::new(Mutex::new(Box::new(writer) as Box<dyn Write + Send>)),
        // Not an ACP session: no negotiated structured route. The static one
        // holds Claude's own frame and the mode gate it goes through.
        image_sink: None,
        static_image_sink: Some(static_prompt),
        reader: Box::new(stdout),
        reader_dispatch: Some(Box::new(reader_dispatch)),
        stderr: Some(Box::new(stderr_source)),
        permission_broker: Some(permission_broker),
        os_handle,
        peer_session_id: resume_peer,
        agent_version: None,
        // The delivery was applied inside `spawn_process`, before this value
        // existed; nothing is left for the session reader to answer.
        pending_delivery: None,
        pending_codex_verify: None,
        out_of_band: None,
    })
}

fn terminate_process(process: &mut Child) {
    let _ = process.kill();
    let _ = process.wait();
}

fn claude_permission_sender(
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    controls: Arc<Mutex<HashMap<u64, ClaudePendingControl>>>,
) -> Arc<PermissionSender> {
    Arc::new(move |id, result| {
        let frame = {
            let controls = controls
                .lock()
                .map_err(|_| io::Error::other("Claude permission map lock poisoned"))?;
            let pending = controls.get(&id);
            let Some(pending) = pending else {
                return Err(io::Error::other(
                    "Claude permission response had no matching control request",
                ));
            };
            control_response_frame(&pending.request_id, &pending.input, &result)
        };
        let mut bytes = serde_json::to_vec(&frame)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        bytes.push(b'\n');
        let result = write_child_stdin(&stdin, &bytes, "Claude");
        if result.is_ok() {
            controls
                .lock()
                .map_err(|_| io::Error::other("Claude permission map lock poisoned"))?
                .remove(&id);
        }
        result
    })
}

fn control_response_frame(request_id: &str, input: &Value, result: &Value) -> Value {
    let outcome = result
        .pointer("/outcome/outcome")
        .and_then(Value::as_str)
        .unwrap_or("");
    let option_id = result
        .pointer("/outcome/optionId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let response = if outcome == "selected" && option_id == "allow" {
        serde_json::json!({
            "behavior": "allow",
            "updatedInput": input,
        })
    } else {
        let message = match outcome {
            "cancelled" => "The permission request was cancelled.",
            _ => "The user declined this command.",
        };
        serde_json::json!({
            "behavior": "deny",
            "message": message,
        })
    };
    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": response,
        }
    })
}

fn frame_user_message(
    text: &str,
    uuid: Option<&str>,
    priority: Option<&str>,
) -> io::Result<Vec<u8>> {
    let mut frame = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": text}]
        }
    });
    if let Some(uuid) = uuid {
        frame["uuid"] = serde_json::Value::String(uuid.to_string());
    }
    if let Some(priority) = priority {
        frame["priority"] = serde_json::Value::String(priority.to_string());
    }
    let mut bytes = serde_json::to_vec(&frame)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// One user message with the Paseo-measured image shape: a content entry of
/// `{"type": "image", ...}` for every carried raster, after the single text
/// entry. Measured on Paseo's own Claude provider (`toSdkUserMessage`): a
/// nested `source` with key `media_type`, not the ACP flat
/// `{type, mimeType, data}`.
///
/// With no blocks this is [`frame_user_message`] byte for byte — the two
/// differ only by the loop that runs zero times — which is what lets the
/// static route frame every prompt through this one builder.
fn frame_user_message_with_images(
    text: &str,
    images: &[super::AcpImageBlock],
) -> io::Result<Vec<u8>> {
    let mut content = Vec::with_capacity(images.len().saturating_add(1));
    content.push(serde_json::json!({"type": "text", "text": text}));
    for image in images {
        content.push(claude_image_block(&image.mime_type, &image.data_base64));
    }
    let frame = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": content
        }
    });
    let mut bytes = serde_json::to_vec(&frame)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// One Claude image content entry: the nested `source` shape above. The
/// label is trusted the way the shared plan trusts it: `materialize` refused
/// any file whose bytes and label disagree, so the bytes on disk are this
/// container.
fn claude_image_block(mime_type: &str, data_base64: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": mime_type,
            "data": data_base64,
        }
    })
}

/// Whether Claude accepts this raster inline. Measured as Paseo's
/// `isImageMimeType`: jpeg/png/gif/webp go in the block, anything else
/// keeps its `[Image available at: ...]` path line — never the silent drop
/// Paseo's Claude provider performs.
///
/// At the call site this is ANDed with
/// [`crate::raster_metadata::RasterMime::from_mime_type`], which knows only
/// jpeg and png, so a gif and a webp — both in the set above, both of which
/// Claude itself accepts — always take the path line instead.
///
/// That is deliberate and must stay: we send inline only what we can prove we
/// stripped. The strip walk recognises two containers on purpose
/// (`raster_metadata.rs` explains that guessing a container from an unknown
/// byte would be inventing a rule rather than applying one), so widening this
/// gate without widening the walk first would ship un-stripped metadata to a
/// provider. The gap is unreachable from the UI today — the Design composer
/// accepts png, jpeg and svg only (`designAttachments.ts`) — and that is
/// context, not a licence to relax it: attachments will arrive from paired
/// devices later, where nothing narrows the set.
fn claude_accepts_inline(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    )
}

/// Prompt plan for one Claude send: the text plus any image blocks land as
/// one `content[]` array. `fallback_text` is the user's text with the path
/// lines for the attachments that stay out of the blocks: SVG, plus any
/// raster Paseo's `isImageMimeType` would refuse. The bytes each block
/// carries are the stripped bytes read back from the file `materialize`
/// wrote, never the base64 that arrived on the wire.
///
/// `plan_claude_prompt` builds it and `ClaudeStaticPrompt` carries it to the
/// frame builder and the mode gate. It travels whole (never `text` and
/// `images` separately) so the text block the child receives and the string
/// the journal records are the same value by construction.
struct ClaudePromptPlan {
    fallback_text: String,
    images: Vec<super::AcpImageBlock>,
}

/// The delivery Claude is authorised for: a fact about the protocol, not a
/// fact the peer agreed to — there is no handshake to negotiate with. Read
/// through the shared enum, not compared against a literal, so a later change
/// to what "statically known" authorises cannot silently re-route this
/// sender. `pub(super)` for the provider trait's `image_delivery`
/// delegation (`provider.rs`) — the fact stays in the family module.
pub(super) fn claude_delivery() -> super::ImageDelivery {
    super::ImageDelivery::StaticImageBlock
}

/// Splits one request's attachments into inline image blocks and path-line
/// fallbacks, materializing each attachment exactly once — the call the shared
/// `with_attachment_paths` makes — so a request that fails on its third
/// attachment leaves nothing half-built.
///
/// `None` means the route did not run at all: no attachments, or a delivery
/// this sender is not authorised for. When it does run it answers with the
/// text as well, even if no raster became a block — an SVG, or a gif whose
/// container no walk follows. That is what keeps the caller from walking the
/// attachments a second time, and it costs no wire change: with no block the
/// frame the route sends is the text-only frame, byte for byte. (The ACP plan
/// next door answers `None` in that case instead, because `Some` there would
/// swap the plain-text write for a structured content array.)
fn plan_claude_prompt(
    store: &AttachmentStore,
    session_id: &str,
    text: &str,
    attachments: &[devboule_protocol::PromptAttachment],
) -> Result<Option<ClaudePromptPlan>, devboule_protocol::WireError> {
    if attachments.is_empty() {
        return Ok(None);
    }
    // The static gate, read through the shared enum so a later change to
    // what "statically known" authorises cannot silently re-route this
    // sender: only the reserved variant frames bytes.
    if claude_delivery() != super::ImageDelivery::StaticImageBlock {
        return Ok(None);
    }
    let session = store.session(session_id).ok_or_else(|| {
        devboule_protocol::WireError::new(
            devboule_protocol::ErrorCode::InvalidRequest,
            "Invalid session id.",
        )
    })?;
    let mut images = Vec::new();
    let mut fallback_paths = Vec::new();
    for attachment in attachments {
        let path = session.materialize(attachment)?;
        // Inline needs both halves: Paseo's measured accept set AND a daemon
        // strip walk for the container. gif/webp have the first but not the
        // second — no walk exists, so the bytes on disk are unstripped and
        // the honest answer is the path line, never an inline block.
        if claude_accepts_inline(&attachment.mime_type)
            && crate::raster_metadata::RasterMime::from_mime_type(&attachment.mime_type).is_some()
        {
            images.push(
                super::AcpImageBlock::from_stored_file(&path, &attachment.mime_type).map_err(
                    |error| {
                        devboule_protocol::WireError::new(
                            devboule_protocol::ErrorCode::Io,
                            format!("Could not read a stored attachment: {error}"),
                        )
                    },
                )?,
            );
        } else {
            fallback_paths.push(path);
        }
    }
    Ok(Some(ClaudePromptPlan {
        fallback_text: super::prompt_text_with_fallback_paths(text, &fallback_paths),
        images,
    }))
}

/// The read-side twin of [`claude_accepts_inline`]: what the daemon kept on
/// disk for a carried block. Used by tests to pin the routing rule without
/// spawning a child.
#[cfg(test)]
fn carried_mime_types(plan: Option<&ClaudePromptPlan>) -> Vec<&str> {
    plan.map(|plan| {
        plan.images
            .iter()
            .map(|image| image.mime_type.as_str())
            .collect()
    })
    .unwrap_or_default()
}

/// The static prompt route for Claude: it plans, frames and writes through
/// the same mode gate the plain-text writer uses. The frame is Claude's own,
/// and it is built on this side of the seam because the gate that orders
/// prompt frames lives here.
pub(crate) struct ClaudeStaticPrompt {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    mode_gate: Option<ClaudeModeGateRef>,
}

impl ClaudeStaticPrompt {
    fn new(stdin: Arc<Mutex<Option<ChildStdin>>>, mode_gate: Option<ClaudeModeGateRef>) -> Self {
        Self { stdin, mode_gate }
    }
}

impl super::StaticImageSink for ClaudeStaticPrompt {
    fn plan_prompt(
        &self,
        store: &AttachmentStore,
        session_id: &str,
        text: &str,
        attachments: &[devboule_protocol::PromptAttachment],
    ) -> Result<Option<Box<dyn super::PlannedStaticPrompt>>, WireError> {
        let Some(plan) = plan_claude_prompt(store, session_id, text, attachments)? else {
            return Ok(None);
        };
        Ok(Some(Box::new(ClaudePlannedPrompt {
            stdin: Arc::clone(&self.stdin),
            mode_gate: self.mode_gate.clone(),
            plan,
        })))
    }
}

/// One planned Claude prompt, ready to frame. It carries the plan whole, so
/// the text on the wire and the text the caller journals cannot be two
/// different strings.
struct ClaudePlannedPrompt {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    mode_gate: Option<ClaudeModeGateRef>,
    plan: ClaudePromptPlan,
}

impl super::PlannedStaticPrompt for ClaudePlannedPrompt {
    fn text(&self) -> &str {
        &self.plan.fallback_text
    }

    /// The references join this plan's text as path lines, after the SVG-style
    /// fallback lines its own attachments left there. They are never image
    /// blocks on this route either, even though Claude carries images inline:
    /// see `session::push_reference_path_lines`.
    fn append_reference_path_lines(&mut self, reference_paths: &[std::path::PathBuf]) {
        super::push_reference_path_lines(&mut self.plan.fallback_text, reference_paths);
    }

    fn send(&self) -> Result<(), WireError> {
        let bytes = frame_user_message_with_images(&self.plan.fallback_text, &self.plan.images)
            .map_err(send_failure)?;
        write_gated_frame(&self.stdin, self.mode_gate.as_ref(), bytes).map_err(send_failure)
    }
}

/// The refusal the plain-text write path already produces for a send that did
/// not reach the child, so a failed structured send reads the same wherever it
/// came from.
fn send_failure(error: io::Error) -> WireError {
    WireError::new(
        ErrorCode::Io,
        format!("Could not send input to the terminal: {error}"),
    )
}

struct ClaudeWriter {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pending: Vec<u8>,
    mode_gate: Option<ClaudeModeGateRef>,
}

impl Write for ClaudeWriter {
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
        // The text-only frame, unchanged. The static route frames its prompts
        // through `frame_user_message_with_images`, which is these same bytes
        // when it carries no block; both then go through `write_gated_frame`.
        let bytes = frame_user_message(&text, None, None)?;
        write_gated_frame(&self.stdin, self.mode_gate.as_ref(), bytes)
    }
}

/// Writes one already-framed prompt through the mode gate: a released gate
/// writes straight to the child, and a frame that arrives while the initial
/// mode response is still outstanding is queued on the gate instead, in
/// arrival order, for the mode handler to release.
///
/// Both the plain-text writer and the static prompt route send through here,
/// so an image frame cannot jump a text frame typed before it, and so the gate
/// has exactly one implementation.
fn write_gated_frame(
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    mode_gate: Option<&ClaudeModeGateRef>,
    bytes: Vec<u8>,
) -> io::Result<()> {
    if let Some(mode_gate) = mode_gate {
        let mut gate = mode_gate
            .lock()
            .map_err(|_| io::Error::other("Claude mode gate lock poisoned"))?;
        match &gate.state {
            ClaudeModeGateState::Ready => {}
            ClaudeModeGateState::Failed(message) => {
                return Err(io::Error::other(message.clone()));
            }
            ClaudeModeGateState::AwaitingResponse { .. } => {
                gate.pending_frames.push(bytes);
                return Ok(());
            }
        }
    }
    write_child_stdin(stdin, &bytes, "Claude")
}

/// Writes one already-framed *steer* through the mode gate, refusing instead of
/// queueing while the gate is still awaiting the initial mode response (A2-01).
///
/// The gate's queue exists for the frames that start a session — the initial
/// mode request and the prompt behind it — and for those it is correct: they
/// are one ordered batch that `flush_gate_frames` writes together, ahead of
/// anything else, and a gate that fails drops the whole batch rather than
/// delivering it late. A *steer* that lands in that queue is a different thing:
/// it is answered `Ok(true)`, which tells the caller its bytes reached the
/// provider inside the running turn, when in fact the provider has not yet
/// been sent a prompt at all. The honest answer for that window is the same
/// refusal every other unavailable steer gets — `Ok(false)`, and the caller's
/// pre-existing fallback — so this route does not queue.
///
/// The decision and the write are one critical section under the gate lock, so
/// the gate cannot release between the check and the write: once it is `Ready`
/// it stays `Ready` (only `Failed` follows, which is an error here, as it is
/// for every other frame through `write_gated_frame`).
fn write_gated_steer_frame(
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    mode_gate: Option<&ClaudeModeGateRef>,
    bytes: &[u8],
) -> io::Result<bool> {
    if let Some(mode_gate) = mode_gate {
        // The guard is held across the write below: the decision and the write
        // are one critical section, which is what the comment above claims
        // (S4-09). `flush_gate_frames` takes the same order — gate, then stdin.
        let gate = mode_gate
            .lock()
            .map_err(|_| io::Error::other("Claude mode gate lock poisoned"))?;
        match &gate.state {
            ClaudeModeGateState::Ready => {}
            ClaudeModeGateState::Failed(message) => {
                return Err(io::Error::other(message.clone()));
            }
            ClaudeModeGateState::AwaitingResponse { .. } => return Ok(false),
        }
        write_child_stdin(stdin, bytes, "Claude")?;
        return Ok(true);
    }
    write_child_stdin(stdin, bytes, "Claude")?;
    Ok(true)
}

struct ClaudeKiller {
    process: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    permission_broker: Arc<PermissionBroker>,
    cancelled: Arc<AtomicBool>,
}

struct ClaudeSwitcher {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    mode_responses: ClaudeModeResponses,
    mode_gate: Option<ClaudeModeGateRef>,
}

struct ClaudeSteerer {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    mode_gate: Option<ClaudeModeGateRef>,
}

impl SessionSteerer for ClaudeSteerer {
    fn steer_active_turn(
        &mut self,
        text: &str,
        _turn: &mut TurnToken<'_>,
    ) -> Result<bool, WireError> {
        if text.trim_start().starts_with('/') {
            return Ok(false);
        }
        let uuid = uuid::Uuid::new_v4().to_string();
        let bytes = frame_user_message(text, Some(&uuid), Some("next")).map_err(send_failure)?;
        // The gate is refused, not queued, while it is `AwaitingResponse`
        // (A2-01). That window is between spawn and the initial mode response:
        // Claude has not been sent the mode request's answer — let alone a
        // prompt — so no turn was ever started for the steer to join, and the
        // caller's answer has to be the refusal every unavailable steer gets,
        // with its pre-existing fallback, not `Ok(true)` for bytes sitting in a
        // queue. The queue itself stays, for the frames that legitimately start
        // the session: `write_gated_frame` still holds them in one ordered
        // batch that a single `flush_gate_frames` writes, and a gate that fails
        // drops the whole batch rather than delivering it late
        // (`fail_initial_mode_parts`). The trace is in
        // `slice-4-daemon-fix-2-report.md`.
        if write_gated_steer_frame(&self.stdin, self.mode_gate.as_ref(), &bytes)
            .map_err(send_failure)?
        {
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(Self {
            stdin: Arc::clone(&self.stdin),
            mode_gate: self.mode_gate.clone(),
        })
    }
}

/// Build a Claude control_request frame.
fn interrupt_frame_bytes(request_id: &str) -> Option<Vec<u8>> {
    control_request_frame_bytes(request_id, serde_json::json!({"subtype": "interrupt"}))
}

fn control_request_frame_bytes(request_id: &str, request: Value) -> Option<Vec<u8>> {
    let frame = serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": request,
    });
    let mut bytes = serde_json::to_vec(&frame).ok()?;
    bytes.push(b'\n');
    Some(bytes)
}

fn start_initial_mode(
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    next_id: &AtomicU64,
    requested_mode: String,
) -> io::Result<ClaudeModeGateRef> {
    let request_id = format!(
        "initial-permission-mode-{}",
        next_id.fetch_add(1, Ordering::Relaxed)
    );
    let mode_gate = Arc::new(Mutex::new(ClaudeModeGate {
        state: ClaudeModeGateState::AwaitingResponse {
            request_id: request_id.clone(),
            requested_mode: requested_mode.clone(),
        },
        pending_frames: Vec::new(),
    }));
    let bytes = control_request_frame_bytes(
        &request_id,
        serde_json::json!({
            "subtype": "set_permission_mode",
            "mode": requested_mode,
        }),
    )
    .ok_or_else(|| io::Error::other("Could not encode Claude mode request."))?;
    if let Err(error) = write_child_stdin(stdin, &bytes, "Claude") {
        if let Ok(mut gate) = mode_gate.lock() {
            gate.state = ClaudeModeGateState::Failed(error.to_string());
        }
        return Err(error);
    }
    Ok(mode_gate)
}

/// The thinking option the profile named, delivered as the same effort
/// control frame the runtime switch uses — written **synchronously**, next
/// to the mode frame written just before it. Both frames are in the pipe
/// before any reader exists, and the gate opens only when the session reader
/// delivers the mode response, so the CLI reads mode, effort, prompts, in
/// pipe order; nothing can race the flush (`start_initial_mode`'s write is
/// synchronous for the same reason).
///
/// The request is registered in `delivery_settings` before the write, so the
/// session reader can route the CLI's answer: success retires it, and a
/// refusal fails the session — a refused effort is a refused card promise,
/// not a silence. The request id is returned for tests and for the reader's
/// pending map.
fn send_initial_effort(
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    next_id: &AtomicU64,
    delivery_settings: &ClaudeDeliverySettings,
    effort: &str,
) -> io::Result<String> {
    send_initial_flag_settings(
        stdin,
        next_id,
        delivery_settings,
        "initial-effort",
        serde_json::json!({ "effortLevel": effort }),
        &format!("thinking option '{effort}'"),
    )
}

/// The fast-mode flag the profile ticked, delivered on the one control frame
/// this client already writes for the thinking option. Paseo applies Claude's
/// `fast_mode` through its SDK's `applyFlagSettings({ fastMode })`; this
/// family speaks the frame that call turns into, so the setting key and the
/// frame shape are copied and the road is the one already proven for
/// `effortLevel`.
///
/// Like the effort it shares the registration with: a CLI that answers this
/// with an error fails the session rather than running unflagged, because a
/// stored tick the CLI will not take is a card that promised a configuration
/// nobody started.
fn send_initial_fast_mode(
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    next_id: &AtomicU64,
    delivery_settings: &ClaudeDeliverySettings,
    on: bool,
) -> io::Result<String> {
    send_initial_flag_settings(
        stdin,
        next_id,
        delivery_settings,
        "initial-fast",
        serde_json::json!({ "fastMode": on }),
        "fast mode",
    )
}

/// The shared bounded read speaks of "the ACP agent". On this road it is the
/// CLI that stayed mute, and a sentence naming the wrong provider is worse than
/// no sentence at all: the human reads it and goes looking at the wrong thing.
/// The bound, the refusal and the overflow rule are the shared ones; only the
/// nouns are this family's.
fn delivery_wait_error(error: WireError) -> WireError {
    if error.code != ErrorCode::Io {
        return error;
    }
    WireError::new(
        ErrorCode::Io,
        error.message.replace("the ACP agent", "Claude").replace(
            "the creation is refused rather than awaited without end",
            "the creation is refused rather than left on a flag nobody confirmed",
        ),
    )
}

/// A duration for a sentence: seconds above a second, milliseconds below. A
/// sub-second bound printed as "0s" reads like an unset timeout, and the wait
/// this names is exactly the bound a test passes to prove the timeout arm works.
fn human_duration(budget: Duration) -> String {
    if budget < Duration::from_secs(1) {
        return format!("{} ms", budget.as_millis());
    }
    format!("{} s", budget.as_secs())
}

/// Drop a delivery registration without answering it, so the session reader
/// never treats a settled request as still owed.
fn retire(delivery_settings: &ClaudeDeliverySettings, request_id: &str) {
    let _ = delivery_settings
        .lock()
        .map(|mut settings| settings.remove(request_id));
}

/// Wait for the answer to one delivered flag-settings request, on the create
/// path, before the child becomes a session.
///
/// Every line read on the way is appended to `prelude` and replayed to the
/// session reader afterwards, so the init event, the mode response and anything
/// else the CLI wrote first are handled by the one parser that understands them
/// — a second reader of the same bytes is where a delivery confirmation and the
/// mode gate would disagree about who saw what.
///
/// Three outcomes, and only one of them accepts a child:
/// - `success` for this request id: delivered, accepted.
/// - `error` for it: the CLI will not take the setting, so the creation is
///   refused. This is the same sentence the async reader publishes for a
///   refusal it sees later; here it stops the child existing at all.
/// - no answer inside `budget`: refused too. A timeout is the dangerous case —
///   the CLI may still apply the setting or not, and a child started on an
///   unconfirmed flag is a card whose promise nobody checked.
fn confirm_delivery_settings(
    reader: &mut std::io::BufReader<ChildStdout>,
    prelude: &mut Vec<u8>,
    request_id: &str,
    delivery_settings: &ClaudeDeliverySettings,
    budget: Duration,
) -> Result<(), WireError> {
    let deadline = Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            retire(delivery_settings, request_id);
            // The wait is named in the unit that can actually describe it: a
            // sub-second bound said "0s" and read like an unset timeout.
            return Err(WireError::new(
                ErrorCode::Io,
                format!(
                    "Claude did not answer the delivered fast mode within {}; the creation is refused rather than left on a flag nobody confirmed",
                    human_duration(budget)
                ),
            ));
        }
        // The crate's one bounded read of a child's pipe, shared with the ACP
        // creation path. `fill_buf` on an empty pipe **blocks**, so a deadline
        // checked around it is unreachable: an earlier shape of this function
        // polled `fill_buf` and reached its "timeout" only when the child
        // happened to write or die — measured as a 200 ms bound that took
        // nineteen minutes in the test below. The shared read peeks the pipe
        // where a peek exists and consults the deadline on every turn; where no
        // peek exists it says so in its own doc instead of pretending.
        let text = match crate::session::acp_client::read_line_bounded(reader, deadline, budget) {
            Ok(line) => line,
            Err(error) => {
                retire(delivery_settings, request_id);
                return Err(delivery_wait_error(error));
            }
        };
        prelude.extend_from_slice(text.as_bytes());
        let Ok(value) = serde_json::from_str::<Value>(text.trim_end()) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("control_response")
            || value
                .pointer("/response/request_id")
                .and_then(Value::as_str)
                != Some(request_id)
        {
            continue;
        }
        // The answer is ours to consume, so retire the registration: the replay
        // through the session reader sees an unknown request id and ignores it,
        // which is what it already does for any control response nobody asked for.
        let names = delivery_settings
            .lock()
            .ok()
            .and_then(|mut settings| settings.remove(request_id));
        let names = names.unwrap_or_else(|| "setting".to_string());
        return match value.pointer("/response/subtype").and_then(Value::as_str) {
            Some("success") => Ok(()),
            Some("error") => Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "Claude refused the delivered {names}: {}; the creation is refused rather than started without it",
                    value
                        .pointer("/response/error")
                        .and_then(Value::as_str)
                        .unwrap_or("no reason given")
                ),
            )),
            _ => Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "Claude returned an invalid response to the delivered {names}; the creation is refused"
                ),
            )),
        };
    }
}

/// The one writer of `apply_flag_settings` at start. `names` is what the
/// session reader puts in its refusal when the CLI answers with an error: a
/// bare setting key would say `{"fastMode":false}` was refused where the
/// human needs to read that the profile's fast mode was.
fn send_initial_flag_settings(
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    next_id: &AtomicU64,
    delivery_settings: &ClaudeDeliverySettings,
    id_prefix: &str,
    settings: Value,
    names: &str,
) -> io::Result<String> {
    let request_id = format!("{id_prefix}-{}", next_id.fetch_add(1, Ordering::Relaxed));
    delivery_settings
        .lock()
        .map_err(|_| io::Error::other("Claude delivery settings map lock poisoned"))?
        .insert(request_id.clone(), names.to_string());
    let bytes = control_request_frame_bytes(
        &request_id,
        serde_json::json!({
            "subtype": "apply_flag_settings",
            "settings": settings,
        }),
    )
    .ok_or_else(|| io::Error::other("Could not encode Claude settings request."))?;
    if let Err(error) = write_child_stdin(stdin, &bytes, "Claude") {
        if let Ok(mut settings) = delivery_settings.lock() {
            settings.remove(&request_id);
        }
        return Err(error);
    }
    Ok(request_id)
}

/// Flush the prompts queued behind the gate and mark it Ready. The caller
/// holds the gate lock; a write failure leaves the gate awaiting so the
/// caller can fail it through the normal path.
fn flush_gate_frames(
    gate: &mut ClaudeModeGate,
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    view: &mut ClaudeView,
    mode_id: &str,
) -> Option<io::Error> {
    view.set_mode(mode_id);
    for bytes in std::mem::take(&mut gate.pending_frames) {
        if let Err(error) = write_child_stdin(stdin, &bytes, "Claude") {
            return Some(error);
        }
    }
    gate.state = ClaudeModeGateState::Ready;
    None
}

fn fail_initial_mode_parts(
    mode_gate: &ClaudeModeGateRef,
    stdin: Option<&Arc<Mutex<Option<ChildStdin>>>>,
    runtime: &Arc<SessionRuntime>,
    expected_request_id: Option<&str>,
    require_awaiting: bool,
    message: &str,
) -> bool {
    let should_publish = match mode_gate.lock() {
        Ok(mut gate) => {
            let can_fail = match &gate.state {
                ClaudeModeGateState::Failed(_) => false,
                ClaudeModeGateState::Ready => !require_awaiting,
                ClaudeModeGateState::AwaitingResponse { request_id, .. } => expected_request_id
                    .map(|expected| expected == request_id)
                    .unwrap_or(true),
            };
            if can_fail {
                drop(std::mem::take(&mut gate.pending_frames));
                gate.state = ClaudeModeGateState::Failed(message.to_string());
                true
            } else {
                false
            }
        }
        Err(_) => expected_request_id.is_none(),
    };
    if !should_publish {
        return false;
    }
    if let Some(stdin) = stdin {
        if let Ok(mut stdin) = stdin.lock() {
            *stdin = None;
        }
    }
    let _ = runtime.publish_agent_event(
        SessionEvent::AgentError {
            message: message.to_string(),
        },
        None,
    );
    true
}

/// The frame write happens on a spawned thread because a full stdin pipe
/// would otherwise block the caller holding the session lock path.
fn send_interrupt_frame(stdin: Arc<Mutex<Option<ChildStdin>>>, request_id: String) {
    let _ = std::thread::Builder::new()
        .name("claude-interrupt".to_string())
        .spawn(move || {
            if let Some(bytes) = interrupt_frame_bytes(&request_id) {
                let _ = write_child_stdin(&stdin, &bytes, "Claude");
            }
        });
}

fn send_control_request_frame(
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    request_id: String,
    request: Value,
) {
    let _ = std::thread::Builder::new()
        .name("claude-set-model".to_string())
        .spawn(move || {
            if let Some(bytes) = control_request_frame_bytes(&request_id, request) {
                let _ = write_child_stdin(&stdin, &bytes, "Claude");
            }
        });
}

impl ModelSwitcher for ClaudeSwitcher {
    fn set_model(&self, model_id: Option<&str>, effort: Option<&str>) -> Result<(), WireError> {
        if let Some(model_id) = model_id {
            let request_id = format!("set-model-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
            send_control_request_frame(
                Arc::clone(&self.stdin),
                request_id,
                serde_json::json!({
                    "subtype": "set_model",
                    "model": model_id,
                }),
            );
        }
        if let Some(effort) = effort {
            let request_id = format!(
                "set-effort-{}",
                self.next_id.fetch_add(1, Ordering::Relaxed)
            );
            send_control_request_frame(
                Arc::clone(&self.stdin),
                request_id,
                serde_json::json!({
                    "subtype": "apply_flag_settings",
                    "settings": {"effortLevel": effort},
                }),
            );
        }
        Ok(())
    }

    fn set_mode(&self, mode_id: &str) -> Result<(), WireError> {
        let request_id = format!(
            "set-permission-mode-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed)
        );
        let (sender, receiver) = mpsc::channel();
        self.mode_responses
            .lock()
            .map_err(|_| WireError::new(ErrorCode::Io, "Claude mode response map is unavailable."))?
            .insert(request_id.clone(), sender);
        let bytes = control_request_frame_bytes(
            &request_id,
            serde_json::json!({
                "subtype": "set_permission_mode",
                "mode": mode_id,
            }),
        )
        .ok_or_else(|| WireError::new(ErrorCode::Io, "Could not encode Claude mode request."))?;
        if let Err(error) = write_child_stdin(&self.stdin, &bytes, "Claude") {
            let _ = self
                .mode_responses
                .lock()
                .map(|mut responses| responses.remove(&request_id));
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not send Claude mode request: {error}"),
            ));
        }
        match receiver.recv_timeout(CONTROL_RESPONSE_TIMEOUT) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(WireError::new(ErrorCode::InvalidRequest, message)),
            Err(error) => {
                let _ = self
                    .mode_responses
                    .lock()
                    .map(|mut responses| responses.remove(&request_id));
                Err(WireError::new(
                    ErrorCode::Io,
                    format!("Claude mode response timed out: {error}"),
                ))
            }
        }
    }

    fn clone_switcher(&self) -> Box<dyn ModelSwitcher> {
        Box::new(Self {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            mode_responses: Arc::clone(&self.mode_responses),
            mode_gate: self.mode_gate.clone(),
        })
    }

    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(ClaudeSteerer {
            stdin: Arc::clone(&self.stdin),
            mode_gate: self.mode_gate.clone(),
        })
    }
}

impl SessionKiller for ClaudeKiller {
    /// Soft interrupt: ask the CLI to abort the current turn. The process,
    /// stdin, and the kill guard stay untouched so later turns keep working.
    fn interrupt(&mut self) {
        // A kill already closed stdin and drained the broker; a late
        // interrupt would only spawn a thread doomed to BrokenPipe.
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        let request_id = format!("interrupt-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        send_interrupt_frame(Arc::clone(&self.stdin), request_id);
        self.permission_broker.cancel_pending();
    }

    fn kill(&mut self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            let request_id = format!("interrupt-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
            send_interrupt_frame(Arc::clone(&self.stdin), request_id);
            self.permission_broker.close();
        }
        if let Ok(mut process) = self.process.lock() {
            let _ = process.kill();
        }
        if let Ok(mut stdin) = self.stdin.lock() {
            *stdin = None;
        }
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self {
            process: Arc::clone(&self.process),
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            permission_broker: Arc::clone(&self.permission_broker),
            cancelled: Arc::clone(&self.cancelled),
        })
    }
}

struct ClaudeReader {
    buffer: Vec<u8>,
    discarding_oversized_line: bool,
    view: ClaudeView,
    permission_broker: Arc<PermissionBroker>,
    controls: Arc<Mutex<HashMap<u64, ClaudePendingControl>>>,
    mode_responses: ClaudeModeResponses,
    /// The delivery's effort requests this reader must answer; empty unless
    /// the spawn wrote an effort frame (see `send_initial_effort`).
    delivery_settings: ClaudeDeliverySettings,
    next_id: Arc<AtomicU64>,
    stdin: Option<Arc<Mutex<Option<ChildStdin>>>>,
    mode_gate: Option<ClaudeModeGateRef>,
    initial_mode_timeout: Duration,
    initial_mode_timer_started: bool,
    initial_mode_timer_cancel: Option<Sender<()>>,
    initial_mode_timer_thread: Option<JoinHandle<()>>,
}

fn observe_mcp_status(value: &Value, runtime: &SessionRuntime) {
    if value.get("type").and_then(Value::as_str) != Some("system")
        || value.get("subtype").and_then(Value::as_str) != Some("init")
    {
        return;
    }
    let Some(server) = value
        .get("mcp_servers")
        .and_then(Value::as_array)
        .and_then(|servers| {
            servers.iter().find(|server| {
                server.get("name").and_then(Value::as_str)
                    == Some(crate::mcp_broker::MCP_SERVER_NAME)
            })
        })
    else {
        return;
    };
    match server.get("status").and_then(Value::as_str) {
        Some("connected") => {
            // The provider frame is only a hint. The broker owns readiness
            // once it has authenticated and served tools/list.
        }
        Some("failed") | Some("error") => {
            runtime.fail_mcp("Claude reported that the MCP broker failed.");
        }
        _ => {}
    }
}

impl ClaudeReader {
    fn new(
        view: ClaudeView,
        permission_broker: Arc<PermissionBroker>,
        controls: Arc<Mutex<HashMap<u64, ClaudePendingControl>>>,
        mode_responses: ClaudeModeResponses,
        next_id: Arc<AtomicU64>,
    ) -> Self {
        Self {
            buffer: Vec::new(),
            discarding_oversized_line: false,
            view,
            permission_broker,
            controls,
            mode_responses,
            delivery_settings: Arc::new(Mutex::new(HashMap::new())),
            next_id,
            stdin: None,
            mode_gate: None,
            initial_mode_timeout: CONTROL_RESPONSE_TIMEOUT,
            initial_mode_timer_started: false,
            initial_mode_timer_cancel: None,
            initial_mode_timer_thread: None,
        }
    }

    fn with_mode_gate(
        view: ClaudeView,
        permission_broker: Arc<PermissionBroker>,
        controls: Arc<Mutex<HashMap<u64, ClaudePendingControl>>>,
        mode_responses: ClaudeModeResponses,
        next_id: Arc<AtomicU64>,
        wiring: ClaudeModeGateWiring,
    ) -> Self {
        let mut reader = Self::new(view, permission_broker, controls, mode_responses, next_id);
        reader.stdin = Some(wiring.stdin);
        reader.mode_gate = Some(wiring.gate);
        reader.initial_mode_timeout = wiring.timeout;
        reader.delivery_settings = wiring.delivery_settings;
        reader
    }

    fn publish(&self, runtime: &SessionRuntime, event: SessionEvent) {
        self.publish_with_seq(runtime, event, None);
    }

    fn publish_with_seq(
        &self,
        runtime: &SessionRuntime,
        event: SessionEvent,
        event_seq: Option<u64>,
    ) {
        let _ = runtime.publish_agent_event_with_seq(event, None, event_seq);
    }

    fn dispatch_line(&mut self, line: &str, runtime: &Arc<SessionRuntime>) {
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(error) => {
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!("Malformed Claude output was skipped: {error}"),
                    },
                );
                return;
            }
        };
        let value = runtime.redact_mcp_value(&value);
        observe_mcp_status(&value, runtime);
        let event_seq = runtime.journal_agent_envelope(&value);
        if self.dispatch_control_response(&value, runtime) {
            return;
        }
        if is_can_use_tool(&value) {
            self.dispatch_permission(&value, runtime, event_seq);
            return;
        }
        for event in self.view.ingest(&value) {
            let event = if matches!(&event, SessionEvent::SessionManifest { .. }) {
                let event = self.manifest_with_requested_mode(event);
                let event = runtime.store_session_manifest(event);
                if let Some(session_id) = self.view.peer_session_id() {
                    runtime.set_peer_session_id(session_id.to_string());
                }
                event
            } else {
                event
            };
            self.publish_with_seq(runtime, event, event_seq);
        }
    }

    fn manifest_with_requested_mode(&self, event: SessionEvent) -> SessionEvent {
        let Some(mode_gate) = &self.mode_gate else {
            return event;
        };
        let requested_mode = mode_gate.lock().ok().and_then(|gate| match &gate.state {
            ClaudeModeGateState::AwaitingResponse { requested_mode, .. } => {
                Some(requested_mode.clone())
            }
            ClaudeModeGateState::Ready | ClaudeModeGateState::Failed(_) => None,
        });
        let Some(requested_mode) = requested_mode else {
            return event;
        };
        match event {
            SessionEvent::SessionManifest {
                provider_id,
                current_model_id,
                models,
                mut modes,
            } => {
                if let Some(modes) = &mut modes {
                    modes.current_mode_id = requested_mode;
                }
                SessionEvent::SessionManifest {
                    provider_id,
                    current_model_id,
                    models,
                    modes,
                }
            }
            event => event,
        }
    }

    fn start_initial_mode_timeout(&mut self, runtime: &Arc<SessionRuntime>) {
        if self.initial_mode_timer_started {
            return;
        }
        self.initial_mode_timer_started = true;
        let Some(mode_gate) = self.mode_gate.as_ref().cloned() else {
            return;
        };
        let Some(stdin) = self.stdin.as_ref().cloned() else {
            return;
        };
        let Some(request_id) = mode_gate.lock().ok().and_then(|gate| match &gate.state {
            ClaudeModeGateState::AwaitingResponse { request_id, .. } => Some(request_id.clone()),
            _ => None,
        }) else {
            return;
        };
        let (cancel_tx, cancel_rx) = mpsc::channel();
        self.initial_mode_timer_cancel = Some(cancel_tx);
        let timer_gate = Arc::clone(&mode_gate);
        let timer_stdin = Arc::clone(&stdin);
        let timer_runtime = Arc::clone(runtime);
        let timer_timeout = self.initial_mode_timeout;
        match std::thread::Builder::new()
            .name("claude-mode-timeout".to_string())
            .spawn(move || {
                if cancel_rx.recv_timeout(timer_timeout).is_err() {
                    fail_initial_mode_parts(
                        &timer_gate,
                        Some(&timer_stdin),
                        &timer_runtime,
                        Some(&request_id),
                        true,
                        "Claude mode response timed out; queued prompt(s) were not delivered because Claude never confirmed the permission mode.",
                    );
                }
            }) {
            Ok(thread) => self.initial_mode_timer_thread = Some(thread),
            Err(_) => {
                self.initial_mode_timer_cancel = None;
                self.fail_initial_mode(runtime, "Could not start Claude mode response timeout.");
            }
        }
    }

    fn cancel_initial_mode_timeout(&mut self) {
        if let Some(cancel) = &self.initial_mode_timer_cancel {
            let _ = cancel.send(());
        }
        if let Some(thread) = self.initial_mode_timer_thread.take() {
            let _ = thread.join();
        }
    }

    fn fail_initial_mode(&mut self, runtime: &Arc<SessionRuntime>, message: &str) {
        self.cancel_initial_mode_timeout();
        if let Some(mode_gate) = &self.mode_gate {
            fail_initial_mode_parts(
                mode_gate,
                self.stdin.as_ref(),
                runtime,
                None,
                false,
                message,
            );
        } else {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: message.to_string(),
                },
            );
        }
    }

    fn complete_initial_mode(
        &mut self,
        runtime: &Arc<SessionRuntime>,
        request_id: &str,
        result: Result<String, String>,
    ) {
        let Some(mode_gate) = self.mode_gate.as_ref().cloned() else {
            return;
        };
        self.cancel_initial_mode_timeout();
        match result {
            Err(message) => {
                fail_initial_mode_parts(
                    &mode_gate,
                    self.stdin.as_ref(),
                    runtime,
                    Some(request_id),
                    true,
                    &message,
                );
            }
            Ok(mode_id) => {
                let write_error = {
                    let Ok(mut gate) = mode_gate.lock() else {
                        self.fail_initial_mode(runtime, "Claude mode gate is unavailable.");
                        return;
                    };
                    if !matches!(
                        &gate.state,
                        ClaudeModeGateState::AwaitingResponse {
                            request_id: expected,
                            ..
                        } if expected == request_id
                    ) {
                        return;
                    }
                    flush_gate_frames(
                        &mut gate,
                        self.stdin.as_ref().expect("mode gate has stdin"),
                        &mut self.view,
                        &mode_id,
                    )
                };
                if let Some(error) = write_error {
                    self.fail_initial_mode(
                        runtime,
                        &format!("Could not send queued Claude prompt: {error}"),
                    );
                }
            }
        }
    }

    fn dispatch_control_response(&mut self, value: &Value, runtime: &Arc<SessionRuntime>) -> bool {
        if value.get("type").and_then(Value::as_str) != Some("control_response") {
            return false;
        }
        let Some(request_id) = value
            .pointer("/response/request_id")
            .and_then(Value::as_str)
        else {
            return true;
        };
        let initial = self.mode_gate.as_ref().and_then(|mode_gate| {
            mode_gate.lock().ok().and_then(|gate| match &gate.state {
                ClaudeModeGateState::AwaitingResponse {
                    request_id: expected,
                    requested_mode,
                } if expected == request_id => Some(requested_mode.clone()),
                _ => None,
            })
        });
        if let Some(requested_mode) = initial {
            let result = match value.pointer("/response/subtype").and_then(Value::as_str) {
                Some("success") => Ok(value
                    .pointer("/response/response/mode")
                    .and_then(Value::as_str)
                    .unwrap_or(&requested_mode)
                    .to_string()),
                Some("error") => Err(value
                    .pointer("/response/error")
                    .and_then(Value::as_str)
                    .unwrap_or("Claude rejected the initial permission mode.")
                    .to_string()),
                _ => Err("Claude returned an invalid permission mode response.".to_string()),
            };
            self.complete_initial_mode(runtime, request_id, result);
            return true;
        }
        // A delivery settings response: the profile's thinking option or fast
        // mode was the card's promise, so a refusal fails the session exactly
        // as a refused initial mode does — an AgentError on the transcript,
        // stdin closed so the child cannot go on to answer anything at the
        // CLI's own level. An ignored response here is the silence the R2a
        // audit's F2 convicted.
        //
        // One arm serves both settings because the frame and its answer are
        // one shape; `delivered_names` is the word the refusal names, so the
        // message says which of the two the CLI would not take.
        let delivered_names = self
            .delivery_settings
            .lock()
            .ok()
            .and_then(|mut settings| settings.remove(request_id));
        if let Some(names) = delivered_names {
            let failure = match value.pointer("/response/subtype").and_then(Value::as_str) {
                Some("success") => None,
                Some("error") => Some(
                    value
                        .pointer("/response/error")
                        .and_then(Value::as_str)
                        .unwrap_or("Claude rejected the delivered setting.")
                        .to_string(),
                ),
                _ => Some(
                    "Claude returned an invalid response to the delivered setting.".to_string(),
                ),
            };
            if let Some(error) = failure {
                let message = format!(
                    "Claude refused the delivered {names}: {error}; the child is being torn down rather than left on the CLI's own setting."
                );
                if let Some(mode_gate) = &self.mode_gate {
                    fail_initial_mode_parts(
                        mode_gate,
                        self.stdin.as_ref(),
                        runtime,
                        None,
                        false,
                        &message,
                    );
                } else {
                    self.publish(runtime, SessionEvent::AgentError { message });
                }
            }
            return true;
        }
        let sender = self
            .mode_responses
            .lock()
            .ok()
            .and_then(|mut responses| responses.remove(request_id));
        let Some(sender) = sender else {
            return true;
        };
        let result = match value.pointer("/response/subtype").and_then(Value::as_str) {
            Some("success") => {
                if let Some(mode_id) = value
                    .pointer("/response/response/mode")
                    .and_then(Value::as_str)
                {
                    self.view.set_mode(mode_id);
                }
                Ok(())
            }
            Some("error") => Err(value
                .pointer("/response/error")
                .and_then(Value::as_str)
                .unwrap_or("Claude rejected the permission mode change.")
                .to_string()),
            _ => Err("Claude returned an invalid permission mode response.".to_string()),
        };
        let _ = sender.send(result);
        true
    }

    fn dispatch_permission(
        &mut self,
        value: &Value,
        runtime: &Arc<SessionRuntime>,
        event_seq: Option<u64>,
    ) {
        let Some(request_id) = value.get("request_id").and_then(Value::as_str) else {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: "Claude permission request had no request_id.".to_string(),
                },
            );
            return;
        };
        let Some(request) = value.get("request") else {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: "Claude permission request had no request body.".to_string(),
                },
            );
            return;
        };
        let tool_name = request
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or("tool");
        let display_name = request
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or(tool_name);
        let tool_use_id = request
            .get("tool_use_id")
            .and_then(Value::as_str)
            .unwrap_or(request_id)
            .to_string();
        let input = request.get("input").cloned().unwrap_or(Value::Null);
        // Paseo never shows `decision_reason`: for every tool but
        // AskUserQuestion the permission summary is empty
        // (packages/server/src/server/agent/providers/claude/agent.ts:1055-1077
        // and :4624-4660). `decision_reason` is the permission engine's
        // internal vocabulary ("Contains simple_expansion"), so it is not
        // surfaced at all — the daemon has no debug-log facility. The
        // description is the request-level string when present, else the
        // input's own `description` when it is a string, else nothing.
        let description = request
            .get("description")
            .and_then(Value::as_str)
            .or_else(|| input.get("description").and_then(Value::as_str))
            .map(str::to_string);
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_string);
        let event = SessionEvent::PermissionRequest {
            tool_call_id: tool_use_id,
            title: display_name.to_string(),
            description,
            command,
            args: None,
            cwd: None,
            env: None,
            options: vec![
                PermissionOption {
                    option_id: "allow".to_string(),
                    name: "Allow once".to_string(),
                    kind: "allow_once".to_string(),
                },
                PermissionOption {
                    option_id: "deny".to_string(),
                    name: "Deny".to_string(),
                    kind: "reject_once".to_string(),
                },
            ],
            is_chooser: None,
            // A placeholder the daemon overwrites with the session's stored
            // origin before the request leaves for a subscriber.
            origin: devboule_protocol::SessionOrigin::unknown(),
            create_agent: None,
        };
        let acp_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut controls) = self.controls.lock() {
            controls.insert(
                acp_id,
                ClaudePendingControl {
                    request_id: request_id.to_string(),
                    input: input.clone(),
                },
            );
        }
        if let Err(error) = self
            .permission_broker
            .register(acp_id, event.clone(), runtime)
        {
            let _ = self.permission_broker.send(
                acp_id,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
            );
            if !matches!(error, PermissionResponseError::AlreadyRecorded) {
                // The repeat-refusal's one plain notice is already up; only
                // the cancelled frame goes back for that case.
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!("Could not queue Claude permission request: {error}"),
                    },
                );
            }
            return;
        }
        if runtime.permission_delivery_enabled() == Some(false) {
            let _ = self.permission_broker.respond(
                match &event {
                    SessionEvent::PermissionRequest { tool_call_id, .. } => tool_call_id,
                    _ => "",
                },
                devboule_protocol::PermissionOutcome::Deny,
            );
            return;
        }
        let tool_call_id = match &event {
            SessionEvent::PermissionRequest { tool_call_id, .. } => tool_call_id.as_str(),
            _ => return,
        };
        match self.permission_broker.auto_answer(tool_call_id, runtime) {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!(
                            "Could not auto-answer Claude permission request: {error}"
                        ),
                    },
                );
                return;
            }
        }
        self.publish_with_seq(runtime, event, event_seq);
    }
}

impl ReaderDispatch for ClaudeReader {
    fn feed(&mut self, bytes: &[u8], runtime: &Arc<SessionRuntime>) -> Result<(), String> {
        self.start_initial_mode_timeout(runtime);
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
                    self.publish(
                        runtime,
                        SessionEvent::AgentError {
                            message: format!(
                                "Claude input line exceeded {MAX_LINE_BYTES} bytes and was discarded."
                            ),
                        },
                    );
                }
                break;
            };
            let line: Vec<u8> = self.buffer.drain(..=newline).collect();
            if line.len() > MAX_LINE_BYTES {
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!(
                            "Claude input line exceeded {MAX_LINE_BYTES} bytes and was discarded."
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
        self.cancel_initial_mode_timeout();
        if let Some(mode_gate) = &self.mode_gate {
            fail_initial_mode_parts(
                mode_gate,
                self.stdin.as_ref(),
                runtime,
                None,
                true,
                "Claude exited before confirming the permission mode; queued prompt(s) were not delivered because Claude never confirmed the permission mode.",
            );
        }
        self.permission_broker.close();
        if !self.buffer.is_empty() {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: "Claude agent ended with an unterminated output line.".to_string(),
                },
            );
        }
    }
}

fn is_can_use_tool(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("control_request")
        && value
            .get("request")
            .and_then(|request| request.get("subtype"))
            .and_then(Value::as_str)
            == Some("can_use_tool")
}

struct ClaudeStderr {
    state: Arc<Mutex<ClaudeStderrState>>,
    handle: Option<JoinHandle<()>>,
}

struct ClaudeStderrState {
    runtime: Option<Arc<SessionRuntime>>,
    pending: std::collections::VecDeque<String>,
}

impl ClaudeStderr {
    fn start(stderr: ChildStderr) -> io::Result<Self> {
        let state = Arc::new(Mutex::new(ClaudeStderrState {
            runtime: None,
            pending: std::collections::VecDeque::new(),
        }));
        let thread_state = Arc::clone(&state);
        let handle = std::thread::Builder::new()
            .name("session-claude-stderr".to_string())
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
                                        }
                                        None
                                    }
                                }
                                Err(_) => return,
                            };
                            if let Some(runtime) = runtime {
                                let _ = runtime.publish_agent_event(
                                    SessionEvent::AgentStderr {
                                        data: runtime.redact_mcp_text(&line),
                                    },
                                    None,
                                );
                            }
                        }
                        Err(_) => return,
                    }
                }
            })?;
        Ok(Self {
            state,
            handle: Some(handle),
        })
    }
}

impl StderrSource for ClaudeStderr {
    fn spawn(mut self: Box<Self>, runtime: Arc<SessionRuntime>) -> io::Result<JoinHandle<()>> {
        let pending = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| io::Error::other("Claude stderr lock poisoned"))?;
            state.runtime = Some(Arc::clone(&runtime));
            std::mem::take(&mut state.pending)
        };
        for line in pending {
            let _ = runtime.publish_agent_event(
                SessionEvent::AgentStderr {
                    data: runtime.redact_mcp_text(&line),
                },
                None,
            );
        }
        self.handle
            .take()
            .ok_or_else(|| io::Error::other("Claude stderr drain was already consumed"))
    }
}

#[cfg(test)]
#[path = "claude_client_tests.rs"]
mod tests;
