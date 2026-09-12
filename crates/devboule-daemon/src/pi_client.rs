//! Pi RPC stdio adapter for live agent sessions.
//!
//! Pi is intentionally outside the MCP broker slice: its RPC protocol has no
//! MCP server configuration or readiness signal, so Pi sessions do not receive
//! the daemon broker.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use devboule_protocol::{
    ErrorCode, PermissionOption, SessionEvent, SessionModeStateView, SessionModeView, SessionModel,
    SessionModelEffort, WireError,
};
use serde_json::Value;

use super::permission_broker::{PermissionBroker, PermissionSender};
use super::PtyCommand;
use super::{
    write_child_stdin, ModelSwitcher, ReaderDispatch, SessionKiller, SessionRuntime,
    SpawnedSession, StderrSource, StdioWaitableChild,
};
use crate::acp_view::PromptCapabilityState;
use crate::atomic::atomic_write;
use crate::paths::RuntimePaths;
use crate::process_tree::{JobObject, ProcessHandle};
use crate::server::ServerState;

const COMMAND_ENV: &str = "DEVBOULE_PI_COMMAND";
const HANDSHAKE_TIMEOUT_ENV: &str = "DEVBOULE_PI_HANDSHAKE_TIMEOUT_MS";
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_LINE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone, Copy)]
struct PiToolPolicy {
    name: &'static str,
    requires_confirmation: bool,
}

const PI_TOOL_POLICIES: &[PiToolPolicy] = &[
    PiToolPolicy {
        name: "read",
        requires_confirmation: false,
    },
    PiToolPolicy {
        name: "grep",
        requires_confirmation: false,
    },
    PiToolPolicy {
        name: "find",
        requires_confirmation: false,
    },
    PiToolPolicy {
        name: "ls",
        requires_confirmation: false,
    },
    PiToolPolicy {
        name: "write",
        requires_confirmation: true,
    },
    PiToolPolicy {
        name: "bash",
        requires_confirmation: true,
    },
];
static PERMISSION_EXTENSION_COUNTER: AtomicU64 = AtomicU64::new(1);

const PERMISSION_EXTENSION_TEMPLATE: &str = r#"export default function (pi) {
  pi.on("session_start", async (_event, ctx) => {
    ctx.ui.notify("devboule-permission-channel", "info");
  });

  // This mediates tool calls handled by this Pi session. It does not mediate
  // subagent tools or processes launched by bash, including a nested pi -p.
  // User extensions remain enabled and may also register a tool_call hook.
  pi.on("tool_call", async (event, ctx) => {
    const readOnly = new Set(__READ_ONLY_TOOLS__);
    if (readOnly.has(event.toolName)) return;
    const input = event.input ?? {};
    const args = Object.entries(input).map(([key, value]) =>
      `${key}=${typeof value === "string" ? value : JSON.stringify(value)}`
    );
    // Pi 0.85.1 serializes confirm(title, message, opts) positionally and
    // does not spread opts. Keep this object as the first argument: moving
    // command, args, or cwd into opts removes them from the wire and breaks
    // the permission gate.
    const confirmed = await ctx.ui.confirm({
      title: "Devboule permission",
      message: `Allow ${event.toolName}?`,
      command: event.toolName,
      args,
      cwd: ctx.cwd,
    });
    if (!confirmed) {
      return { block: true, reason: "Denied by Devboule permission broker" };
    }
  });
}
"#;

fn permission_extension() -> String {
    let read_only = PI_TOOL_POLICIES
        .iter()
        .filter(|tool| is_read_only_tool(tool.name))
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    let read_only = serde_json::to_string(&read_only).expect("Pi tool policy is serializable");
    PERMISSION_EXTENSION_TEMPLATE.replace("__READ_ONLY_TOOLS__", &read_only)
}

pub(super) fn resolve_command(_paths: &RuntimePaths) -> Result<PtyCommand, WireError> {
    let cwd = std::env::current_dir().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not determine Pi working directory: {error}"),
        )
    })?;
    let mut argv: Vec<String> = match std::env::var(COMMAND_ENV) {
        Ok(argv) => serde_json::from_str(&argv).map_err(|error| {
            WireError::new(
                ErrorCode::InvalidRequest,
                format!("{COMMAND_ENV} must be a non-empty JSON string array: {error}"),
            )
        })?,
        Err(_) => {
            let Some(agent) = crate::provider_catalog::find_available("pi") else {
                return Err(WireError::new(
                    ErrorCode::Io,
                    format!(
                        "Pi was not found on PATH. Set {COMMAND_ENV} to a non-empty JSON string array to choose a command explicitly."
                    ),
                ));
            };
            let Some(rpc) = agent.rpc_command else {
                return Err(WireError::new(
                    ErrorCode::Io,
                    "Pi is installed but has no pi-rpc launch args.",
                ));
            };
            rpc
        }
    };
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("{COMMAND_ENV} must contain an executable."),
        ));
    }
    validate_pi_args(&argv[1..])?;
    let program = argv.remove(0);
    Ok(PtyCommand::new(program, argv, cwd, Vec::new()).with_provider_id("pi"))
}

pub(crate) fn write_permission_extension(path: &std::path::Path) -> io::Result<()> {
    if !path.parent().is_some_and(std::path::Path::is_dir) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Pi permission extension parent directory does not exist",
        ));
    }
    let extension = permission_extension();
    atomic_write(path, extension.as_bytes())
}

fn validate_pi_args(args: &[String]) -> Result<(), WireError> {
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            break;
        }
        if arg == "--mode" {
            let value = args.get(index + 1).map(String::as_str);
            if value != Some("rpc") {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("Pi command must use '--mode rpc', not '{value:?}'."),
                ));
            }
            index += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--mode=") {
            if value != "rpc" {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("Pi command must use '--mode rpc', not '{arg}'."),
                ));
            }
        }
        index += 1;
    }
    Ok(())
}

fn spawn_args(command: &PtyCommand, extension_path: &Path) -> Result<Vec<String>, WireError> {
    validate_pi_args(&command.args)?;
    let mut args = command.args.clone();
    let option_end = |args: &[String]| {
        args.iter()
            .position(|arg| arg == "--")
            .unwrap_or(args.len())
    };
    let has_rpc_mode = args[..option_end(&args)]
        .iter()
        .enumerate()
        .any(|(index, arg)| {
            arg == "--mode=rpc"
                || (arg == "--mode" && args.get(index + 1).is_some_and(|value| value == "rpc"))
        });
    if !has_rpc_mode {
        let index = option_end(&args);
        args.splice(index..index, ["--mode".to_string(), "rpc".to_string()]);
    }
    let index = option_end(&args);
    args.splice(
        index..index,
        [
            "-e".to_string(),
            extension_path.to_string_lossy().into_owned(),
        ],
    );
    Ok(args)
}

fn permission_extension_path(runtime_dir: &Path) -> PathBuf {
    let serial = PERMISSION_EXTENSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    runtime_dir.join(format!("devboule-pi-permissions-{serial}.ts"))
}

fn remove_permission_extension(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != io::ErrorKind::NotFound {
            eprintln!(
                "could not remove Pi permission extension {}: {error}",
                path.display()
            );
        }
    }
}

pub(super) fn spawn_process(
    state: &Arc<ServerState>,
    command: PtyCommand,
    requested_mode: Option<String>,
) -> Result<SpawnedSession, WireError> {
    let mode_id = requested_mode.as_deref().unwrap_or("bypass");
    if !matches!(mode_id, "bypass" | "ask") {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("Pi session mode '{mode_id}' is not available."),
        ));
    }
    let extension_path = permission_extension_path(state.sessions.runtime_dir());
    let args = spawn_args(&command, &extension_path)?;
    if let Err(error) = write_permission_extension(&extension_path) {
        remove_permission_extension(&extension_path);
        return Err(WireError::new(
            ErrorCode::Io,
            format!("Could not write the Pi permission extension: {error}"),
        ));
    }

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
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(error) => {
            remove_permission_extension(&extension_path);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not start Pi {}: {error}", command.program),
            ));
        }
    };

    #[cfg(windows)]
    let (process_job, os_handle) = {
        use std::os::windows::io::AsRawHandle;
        let process_job = JobObject::new().map_err(|error| {
            terminate_process(&mut child);
            remove_permission_extension(&extension_path);
            WireError::new(
                ErrorCode::Io,
                format!("Could not create the Pi process job: {error}"),
            )
        })?;
        let handle = child.as_raw_handle();
        if let Err(error) = state
            .process_job
            .assign(handle)
            .and_then(|()| process_job.assign(handle))
        {
            terminate_process(&mut child);
            remove_permission_extension(&extension_path);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not contain the Pi process: {error}"),
            ));
        }
        let os_handle = match ProcessHandle::duplicate(handle) {
            Ok(duplicated) => Some(duplicated),
            Err(error) => {
                eprintln!("could not duplicate Pi process handle for OS liveness: {error}");
                None
            }
        };
        (process_job, os_handle)
    };

    #[cfg(not(windows))]
    let process_job = JobObject::new().map_err(|error| {
        terminate_process(&mut child);
        remove_permission_extension(&extension_path);
        WireError::new(
            ErrorCode::Io,
            format!("Could not create the Pi process job: {error}"),
        )
    })?;
    #[cfg(not(windows))]
    let os_handle = None;

    let stdin = child.stdin.take().ok_or_else(|| {
        terminate_process(&mut child);
        remove_permission_extension(&extension_path);
        WireError::new(ErrorCode::Io, "Pi did not provide stdin.")
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        terminate_process(&mut child);
        remove_permission_extension(&extension_path);
        WireError::new(ErrorCode::Io, "Pi did not provide stdout.")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        terminate_process(&mut child);
        remove_permission_extension(&extension_path);
        WireError::new(ErrorCode::Io, "Pi did not provide stderr.")
    })?;
    let process = Arc::new(Mutex::new(child));
    let stdin = Arc::new(Mutex::new(Some(stdin)));
    let next_id = Arc::new(AtomicU64::new(1));
    let mut stdout = match PiStdout::spawn(stdout) {
        Ok(stdout) => stdout,
        Err(error) => {
            terminate_shared_process(&process);
            remove_permission_extension(&extension_path);
            drop(process_job);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not read Pi stdout: {error}"),
            ));
        }
    };
    let handshake = match perform_handshake(&mut stdout, &stdin, &next_id, mode_id) {
        Ok(handshake) => handshake,
        Err(error) => {
            terminate_shared_process(&process);
            remove_permission_extension(&extension_path);
            drop(process_job);
            return Err(error);
        }
    };

    let controls = Arc::new(Mutex::new(HashMap::new()));
    let permission_extension_active = Arc::new(AtomicBool::new(
        handshake.deferred.iter().any(is_ready_notify),
    ));
    if mode_id == "ask" && !permission_extension_active.load(Ordering::Acquire) {
        terminate_shared_process(&process);
        remove_permission_extension(&extension_path);
        drop(process_job);
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "Pi permission extension not active.",
        ));
    }
    let permission_broker = PermissionBroker::with_sender(pi_permission_sender(
        Arc::clone(&stdin),
        Arc::clone(&controls),
    ));
    let control = Arc::new(PiControl::new(Arc::clone(&stdin), Arc::clone(&next_id)));
    let writer = PiWriter {
        stdin: Arc::clone(&stdin),
        next_id: Arc::clone(&next_id),
        pending: Vec::new(),
    };
    let killer = PiKiller {
        process: Arc::clone(&process),
        stdin: Arc::clone(&stdin),
        next_id: Arc::clone(&next_id),
        permission_broker: Arc::clone(&permission_broker),
        cancelled: Arc::new(AtomicBool::new(false)),
        extension_path: extension_path.clone(),
    };
    let reader_dispatch = PiReader::new(
        handshake.deferred,
        handshake.manifest,
        Arc::clone(&permission_broker),
        Arc::clone(&controls),
        Arc::clone(&next_id),
        Arc::clone(&control),
        Arc::clone(&stdin),
        Arc::clone(&permission_extension_active),
    )
    .with_extension_path(extension_path.clone());
    let stderr_source = PiStderr::start(stderr).map_err(|error| {
        terminate_shared_process(&process);
        remove_permission_extension(&extension_path);
        WireError::new(ErrorCode::Io, format!("Could not drain Pi stderr: {error}"))
    })?;
    // The static prompt route reads the live model from the same catalog the
    // switcher keeps, so the two share one `Arc`.
    let catalog = Arc::new(Mutex::new(handshake.catalog));
    let static_prompt = Arc::new(PiStaticPrompt::new(
        Arc::clone(&stdin),
        Arc::clone(&next_id),
        Arc::clone(&catalog),
    ));
    Ok(SpawnedSession {
        process_job,
        master: None,
        killer: Box::new(killer),
        switcher: Some(Box::new(PiSwitcher {
            control,
            catalog,
            mode_id: Arc::new(Mutex::new(mode_id.to_string())),
            permission_extension_active,
        })),
        child: Box::new(StdioWaitableChild { process }),
        writer: Arc::new(Mutex::new(Box::new(writer) as Box<dyn Write + Send>)),
        // Not an ACP session: no negotiated structured route. The static one
        // sends Pi's own `prompt` frame.
        image_sink: None,
        static_image_sink: Some(static_prompt),
        reader: Box::new(stdout),
        reader_dispatch: Some(Box::new(reader_dispatch)),
        stderr: Some(Box::new(stderr_source)),
        permission_broker: Some(permission_broker),
        os_handle,
        peer_session_id: handshake.peer_session_id,
        agent_version: None,
    })
}

fn terminate_process(process: &mut Child) {
    let _ = process.kill();
    let _ = process.wait();
}

fn terminate_shared_process(process: &Arc<Mutex<Child>>) {
    let mut process = match process.lock() {
        Ok(process) => process,
        Err(poisoned) => poisoned.into_inner(),
    };
    terminate_process(&mut process);
}

struct Handshake {
    peer_session_id: Option<String>,
    manifest: SessionEvent,
    catalog: PiCatalog,
    deferred: Vec<Value>,
}

fn perform_handshake(
    stdout: &mut PiStdout,
    stdin: &Mutex<Option<ChildStdin>>,
    next_id: &AtomicU64,
    mode_id: &str,
) -> Result<Handshake, WireError> {
    let deadline = Instant::now() + handshake_timeout();
    let mut deferred = Vec::new();
    let mut peer_session_id = None;
    let state = request_response(
        stdout,
        stdin,
        next_id,
        "get_state",
        Value::Null,
        deadline,
        &mut deferred,
    )?;
    if let Some(session_id) = state
        .get("data")
        .and_then(|value| value.get("sessionId"))
        .and_then(Value::as_str)
    {
        peer_session_id = Some(session_id.to_string());
    }
    let models = request_response(
        stdout,
        stdin,
        next_id,
        "get_available_models",
        Value::Null,
        deadline,
        &mut deferred,
    )?;
    let levels = request_response(
        stdout,
        stdin,
        next_id,
        "get_available_thinking_levels",
        Value::Null,
        deadline,
        &mut deferred,
    )?;
    let catalog = catalog_from_responses(&state, &models, &levels)?;
    let manifest = manifest_from_catalog(&catalog, mode_id);
    Ok(Handshake {
        peer_session_id,
        manifest,
        catalog,
        deferred,
    })
}

fn handshake_timeout() -> Duration {
    std::env::var(HANDSHAKE_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .filter(|value| !value.is_zero())
        .unwrap_or(DEFAULT_HANDSHAKE_TIMEOUT)
}

fn handshake_error(reason: &str) -> WireError {
    WireError::new(
        ErrorCode::Io,
        format!("Could not start Pi session: {reason}"),
    )
}

fn request_response(
    stdout: &mut PiStdout,
    stdin: &Mutex<Option<ChildStdin>>,
    next_id: &AtomicU64,
    command: &str,
    extra: Value,
    deadline: Instant,
    deferred: &mut Vec<Value>,
) -> Result<Value, WireError> {
    let id = format!("h-{}", next_id.fetch_add(1, Ordering::Relaxed));
    let mut frame = serde_json::json!({"id": id, "type": command});
    if let Some(object) = extra.as_object() {
        if let Some(frame_object) = frame.as_object_mut() {
            frame_object.extend(object.clone());
        }
    }
    send_json(stdin, &frame, "Pi")?;
    loop {
        let Some(value) = stdout
            .next_line(deadline)
            .map_err(|error| handshake_error(&format!("could not read response: {error}")))?
        else {
            return Err(handshake_error(&format!(
                "Pi exited before {command} completed"
            )));
        };
        let value: Value = serde_json::from_str(&value)
            .map_err(|error| handshake_error(&format!("malformed {command} response: {error}")))?;
        if value.get("id").and_then(Value::as_str) == Some(id.as_str())
            && value.get("type").and_then(Value::as_str) == Some("response")
        {
            if value.get("success").and_then(Value::as_bool) != Some(true) {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "Pi {command} failed: {}",
                        value
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error")
                    ),
                ));
            }
            return Ok(value);
        }
        deferred.push(value);
    }
}

fn send_json(
    stdin: &Mutex<Option<ChildStdin>>,
    value: &Value,
    label: &'static str,
) -> Result<(), WireError> {
    let mut bytes = serde_json::to_vec(value).map_err(|error| {
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

fn is_ready_notify(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("extension_ui_request")
        && value.get("method").and_then(Value::as_str) == Some("notify")
        && value.get("message").and_then(Value::as_str) == Some("devboule-permission-channel")
}

fn session_id_from_value(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) == Some("session") {
        return value.get("id").and_then(Value::as_str).map(str::to_string);
    }
    value
        .get("data")
        .and_then(|data| data.get("sessionId"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[derive(Clone, Debug, Default)]
struct PiCatalog {
    models: HashMap<String, PiModel>,
    current_model_id: Option<String>,
    current_provider: Option<String>,
    current_effort: Option<String>,
    current_levels: Vec<String>,
}

impl PiCatalog {
    /// The content kinds a model declared in `get_available_models`. The read
    /// side for a future image sender: nothing sends one yet, so in production
    /// this is only the precondition this slice installs.
    #[cfg_attr(not(test), allow(dead_code))]
    fn input_kinds(&self, model_id: &str) -> Option<&PiInputKinds> {
        self.models.get(model_id).map(|model| &model.input)
    }
}

#[derive(Clone, Debug)]
struct PiModel {
    name: String,
    provider: Option<String>,
    context_tokens: Option<u64>,
    efforts: Option<Vec<SessionModelEffort>>,
    /// Content kinds this model declared in `get_available_models`. Retained so
    /// a later sender can ask whether the model accepts an image instead of
    /// guessing from its id.
    input: PiInputKinds,
}

/// Content kinds one pi model declared in its `input` array.
///
/// `input` is per model, not per session: models in one catalog disagree, and
/// the captured `get_available_models` reply proves it — `deepseek-v4-flash`
/// declares only `text` while `minimax-m3` declares `text` and `image`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PiInputKinds {
    /// Every token the model declared, verbatim and in wire order, including
    /// kinds this daemon does not model yet. Recognising `image` must not
    /// discard the rest; that is the same rule the ACP view applies to content
    /// blocks it cannot render.
    declared: Vec<String>,
    /// Whether the model accepts images, in three separate states. Reuses the
    /// ACP handshake's tri-state so "declared no image" and "declared nothing"
    /// stay apart here too: a model that never mentioned `input` is not a model
    /// that refused images.
    image: PromptCapabilityState,
}

const IMAGE_INPUT_KIND: &str = "image";

/// Reads a model's `input` array. A missing `input`, or one that is not an
/// array, leaves every kind absent — silence is not a refusal, and a malformed
/// value is not evidence either. A present array is a declaration, so its
/// failure to list `image` is [`PromptCapabilityState::Unsupported`].
fn input_kinds_from_model(model: &Value) -> PiInputKinds {
    let Some(entries) = model.get("input").and_then(Value::as_array) else {
        return PiInputKinds::default();
    };
    let declared = entries
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let image = if declared.iter().any(|kind| kind == IMAGE_INPUT_KIND) {
        PromptCapabilityState::Supported
    } else {
        PromptCapabilityState::Unsupported
    };
    PiInputKinds { declared, image }
}

fn catalog_from_responses(
    state_response: &Value,
    models_response: &Value,
    levels_response: &Value,
) -> Result<PiCatalog, WireError> {
    let data = models_response.get("data").ok_or_else(|| {
        WireError::new(ErrorCode::InvalidRequest, "Pi model response had no data.")
    })?;
    let mut catalog = PiCatalog::default();
    let current_model = state_response
        .get("data")
        .and_then(|data| data.get("model"));
    catalog.current_model_id = current_model
        .and_then(|model| model.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    catalog.current_provider = current_model
        .and_then(|model| model.get("provider"))
        .and_then(Value::as_str)
        .map(str::to_string);
    catalog.current_effort = state_response
        .get("data")
        .and_then(|data| data.get("thinkingLevel"))
        .and_then(Value::as_str)
        .map(str::to_string);
    catalog.current_levels = levels_response
        .get("data")
        .and_then(|data| data.get("levels"))
        .and_then(Value::as_array)
        .map(|levels| {
            levels
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let models = data
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            WireError::new(
                ErrorCode::InvalidRequest,
                "Pi model response had no models.",
            )
        })?;
    for model in models {
        let Some(id) = model.get("id").and_then(Value::as_str) else {
            continue;
        };
        let map_efforts = model
            .get("thinkingLevelMap")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter(|(_, value)| !value.is_null())
                    .map(|(id, _)| effort(id, false))
                    .collect::<Vec<_>>()
            })
            .filter(|efforts| !efforts.is_empty());
        let efforts = if catalog.current_model_id.as_deref() == Some(id) {
            Some(
                catalog
                    .current_levels
                    .iter()
                    .map(|level| effort(level, catalog.current_effort.as_deref() == Some(level)))
                    .collect(),
            )
        } else {
            map_efforts
        };
        catalog.models.insert(
            id.to_string(),
            PiModel {
                name: model
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_string(),
                provider: model
                    .get("provider")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                context_tokens: model.get("contextWindow").and_then(Value::as_u64),
                efforts,
                input: input_kinds_from_model(model),
            },
        );
    }
    if catalog.current_model_id.is_none() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "Pi state did not declare a current model.",
        ));
    }
    Ok(catalog)
}

fn effort(id: &str, default: bool) -> SessionModelEffort {
    SessionModelEffort {
        id: id.to_string(),
        label: id.to_string(),
        description: None,
        default: Some(default),
    }
}

fn manifest_from_catalog(catalog: &PiCatalog, mode_id: &str) -> SessionEvent {
    let mut ids = catalog.models.keys().cloned().collect::<Vec<_>>();
    ids.sort();
    let models = ids
        .into_iter()
        .filter_map(|id| {
            let model = catalog.models.get(&id)?;
            Some(SessionModel {
                model_id: id.clone(),
                name: model.name.clone(),
                description: None,
                context_tokens: model.context_tokens,
                current_effort: (catalog.current_model_id.as_deref() == Some(id.as_str()))
                    .then(|| catalog.current_effort.clone())
                    .flatten(),
                efforts: model.efforts.clone(),
            })
        })
        .collect();
    SessionEvent::SessionManifest {
        provider_id: Some("pi".to_string()),
        current_model_id: catalog.current_model_id.clone(),
        models,
        modes: Some(SessionModeStateView {
            current_mode_id: mode_id.to_string(),
            available_modes: vec![
                SessionModeView {
                    id: "bypass".to_string(),
                    name: "Bypass".to_string(),
                    description: Some(
                        "Tools run without asking (Pi's native behaviour)".to_string(),
                    ),
                },
                SessionModeView {
                    id: "ask".to_string(),
                    name: "Always ask".to_string(),
                    description: Some("Ask before every tool call".to_string()),
                },
            ],
        }),
    }
}

fn pi_permission_sender(
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    controls: Arc<Mutex<HashMap<u64, String>>>,
) -> Arc<PermissionSender> {
    Arc::new(move |id, result| {
        let request_id = controls
            .lock()
            .map_err(|_| io::Error::other("Pi permission map lock poisoned"))?
            .get(&id)
            .cloned()
            .ok_or_else(|| io::Error::other("Pi permission response had no matching request"))?;
        let confirmed = result.pointer("/outcome/outcome").and_then(Value::as_str)
            == Some("selected")
            && result.pointer("/outcome/optionId").and_then(Value::as_str) == Some("allow");
        let frame = serde_json::json!({
            "id": request_id,
            "type": "extension_ui_response",
            "confirmed": confirmed,
        });
        let mut bytes = serde_json::to_vec(&frame)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        bytes.push(b'\n');
        let result = write_child_stdin(&stdin, &bytes, "Pi");
        if result.is_ok() {
            controls
                .lock()
                .map_err(|_| io::Error::other("Pi permission map lock poisoned"))?
                .remove(&id);
        }
        result
    })
}

/// One Pi `images[]` entry: the Paseo-measured `convertPromptInput` shape
/// `{"type": "image", "data": ..., "mimeType": ...}` — flat, with a
/// capital-T `mimeType`, unlike Claude's nested `source`/`media_type`. The
/// bytes are the stripped bytes read back from the file `materialize`
/// wrote, never the base64 that arrived on the wire.
fn pi_image_entry(mime_type: &str, data_base64: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "image",
        "data": data_base64,
        "mimeType": mime_type,
    })
}

/// The `prompt` frame with the optional `images` field: present only when at
/// least one raster travels. Absent otherwise, matching Paseo's
/// `...(images?.length ? { images } : {})` — the child must not see an empty
/// array where the measured sender omits the field.
///
/// With no entries it is the text-only frame the writer has always sent, byte
/// for byte — `a_frame_without_entries_is_the_text_only_frame` pins that —
/// which is what lets the static route send every prompt through it.
fn pi_prompt_frame(id: &str, text: &str, images: &[super::AcpImageBlock]) -> serde_json::Value {
    let mut frame = serde_json::json!({
        "id": id,
        "type": "prompt",
        "message": text,
    });
    if !images.is_empty() {
        let entries = images
            .iter()
            .map(|image| pi_image_entry(&image.mime_type, &image.data_base64))
            .collect::<Vec<_>>();
        frame
            .as_object_mut()
            .expect("prompt frame is an object")
            .insert("images".to_string(), serde_json::Value::Array(entries));
    }
    frame
}

/// Prompt plan for one Pi send: the text plus any image entries. The
/// capability rule is Paseo's `piModelSupportsImageInput` — `image` in the
/// current model's `input` — read through the tri-state this daemon already
/// keeps per model: `Supported` frames bytes, `Unsupported` AND `Absent`
/// keep the path line. A model whose inputs we do not know gets the path
/// line, never an attempt.
///
/// `PiStaticPrompt` sends it as the `prompt` frame above.
struct PiPromptPlan {
    fallback_text: String,
    images: Vec<super::AcpImageBlock>,
}

/// The delivery the current Pi model authorises, read from the catalog the
/// handshake filled — at prompt time, not copied at spawn, so a model switched
/// since then is the model this answers for.
fn pi_delivery(catalog: &PiCatalog, model_id: Option<&str>) -> super::ImageDelivery {
    let image = model_id
        .and_then(|id| catalog.input_kinds(id))
        .map(|kinds| kinds.image)
        .unwrap_or(PromptCapabilityState::Absent);
    match image {
        PromptCapabilityState::Supported => super::ImageDelivery::StaticImageBlock,
        PromptCapabilityState::Unsupported | PromptCapabilityState::Absent => {
            super::ImageDelivery::PathLine
        }
    }
}

/// Splits one request's attachments into inline image entries and path-line
/// fallbacks. Every attachment is materialized first — exactly the call the
/// shared `with_attachment_paths` makes — so a request that fails on its
/// third attachment leaves nothing half-built.
///
/// `None` means the route did not run at all: no attachments, or a model whose
/// `input` does not declare `image` (declared-without-image and not-declared
/// are both no). When it does run it answers with the text as well, even if no
/// raster became an entry, so the caller never walks the attachments a second
/// time; with no entry the frame is the text-only `prompt`, byte for byte.
fn plan_pi_prompt(
    store: &crate::attachment_store::AttachmentStore,
    session_id: &str,
    text: &str,
    attachments: &[devboule_protocol::PromptAttachment],
    catalog: &PiCatalog,
    model_id: Option<&str>,
) -> Result<Option<PiPromptPlan>, devboule_protocol::WireError> {
    if attachments.is_empty() {
        return Ok(None);
    }
    // Unknown is never yes: only a model that declared `image` frames bytes.
    if pi_delivery(catalog, model_id) != super::ImageDelivery::StaticImageBlock {
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
        if crate::raster_metadata::RasterMime::from_mime_type(&attachment.mime_type).is_some() {
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
    Ok(Some(PiPromptPlan {
        fallback_text: super::prompt_text_with_fallback_paths(text, &fallback_paths),
        images,
    }))
}

/// The read-side twin of the routing rule above: what the daemon kept on
/// disk for a carried entry. Used by tests to pin the rule without spawning
/// a child.
#[cfg(test)]
fn carried_pi_mime_types(plan: Option<&PiPromptPlan>) -> Vec<&str> {
    plan.map(|plan| {
        plan.images
            .iter()
            .map(|image| image.mime_type.as_str())
            .collect()
    })
    .unwrap_or_default()
}

/// The static prompt route for Pi: it plans, then sends Pi's own `prompt`
/// frame with the `images[]` field the measured sender uses.
pub(crate) struct PiStaticPrompt {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    catalog: Arc<Mutex<PiCatalog>>,
}

impl PiStaticPrompt {
    fn new(
        stdin: Arc<Mutex<Option<ChildStdin>>>,
        next_id: Arc<AtomicU64>,
        catalog: Arc<Mutex<PiCatalog>>,
    ) -> Self {
        Self {
            stdin,
            next_id,
            catalog,
        }
    }
}

impl super::StaticImageSink for PiStaticPrompt {
    fn plan_prompt(
        &self,
        store: &crate::attachment_store::AttachmentStore,
        session_id: &str,
        text: &str,
        attachments: &[devboule_protocol::PromptAttachment],
    ) -> Result<Option<Box<dyn super::PlannedStaticPrompt>>, WireError> {
        // The catalog is read here, at prompt time: a model switched since
        // spawn must not be answered for with the inputs of the model that was
        // current then.
        let catalog = self
            .catalog
            .lock()
            .map_err(|_| WireError::new(ErrorCode::Io, "Pi model catalog is unavailable."))?;
        let model_id = catalog.current_model_id.clone();
        let Some(plan) = plan_pi_prompt(
            store,
            session_id,
            text,
            attachments,
            &catalog,
            model_id.as_deref(),
        )?
        else {
            return Ok(None);
        };
        Ok(Some(Box::new(PiPlannedPrompt {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            plan,
        })))
    }
}

/// One planned Pi prompt, ready to send. It carries the plan whole, so the
/// text on the wire and the text the caller journals cannot be two different
/// strings.
struct PiPlannedPrompt {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    plan: PiPromptPlan,
}

impl PiPlannedPrompt {
    /// The `prompt` frame for this plan. The request id comes off the shared
    /// counter the writer and the control channel use, taken at send time.
    fn frame(&self) -> serde_json::Value {
        pi_prompt_frame(
            &format!("p-{}", self.next_id.fetch_add(1, Ordering::Relaxed)),
            &self.plan.fallback_text,
            &self.plan.images,
        )
    }
}

impl super::PlannedStaticPrompt for PiPlannedPrompt {
    fn text(&self) -> &str {
        &self.plan.fallback_text
    }

    fn send(&self) -> Result<(), WireError> {
        let mut bytes = serde_json::to_vec(&self.frame()).map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                format!("Could not encode the Pi prompt frame: {error}"),
            )
        })?;
        bytes.push(b'\n');
        write_child_stdin(&self.stdin, &bytes, "Pi").map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                format!("Could not send input to the terminal: {error}"),
            )
        })
    }
}

struct PiWriter {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    pending: Vec<u8>,
}

impl Write for PiWriter {
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
        // The text-only frame, unchanged. A prompt that carries images is
        // sent by the static route's own `prompt` frame instead of by this
        // writer, so `pi_prompt_frame` has one production caller and this
        // literal keeps the other: with no entry the two build the same frame,
        // which `a_frame_without_entries_is_the_text_only_frame` pins.
        let frame = serde_json::json!({
            "id": format!("p-{}", self.next_id.fetch_add(1, Ordering::Relaxed)),
            "type": "prompt",
            "message": text,
        });
        let mut bytes = serde_json::to_vec(&frame)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        bytes.push(b'\n');
        write_child_stdin(&self.stdin, &bytes, "Pi")
    }
}

struct PiKiller {
    process: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    permission_broker: Arc<PermissionBroker>,
    cancelled: Arc<AtomicBool>,
    extension_path: PathBuf,
}

impl PiKiller {
    fn abort(&self) {
        let frame = serde_json::json!({
            "id": format!("a-{}", self.next_id.fetch_add(1, Ordering::Relaxed)),
            "type": "abort",
        });
        if let Ok(mut bytes) = serde_json::to_vec(&frame) {
            bytes.push(b'\n');
            let _ = write_child_stdin(&self.stdin, &bytes, "Pi");
        }
    }
}

impl SessionKiller for PiKiller {
    fn interrupt(&mut self) {
        self.abort();
        self.permission_broker.cancel_pending();
    }

    fn kill(&mut self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            self.abort();
            self.permission_broker.close();
        }
        if let Ok(mut process) = self.process.lock() {
            let _ = process.kill();
        }
        if let Ok(mut stdin) = self.stdin.lock() {
            *stdin = None;
        }
        remove_permission_extension(&self.extension_path);
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self {
            process: Arc::clone(&self.process),
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            permission_broker: Arc::clone(&self.permission_broker),
            cancelled: Arc::clone(&self.cancelled),
            extension_path: self.extension_path.clone(),
        })
    }
}

struct PiControl {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    pending: Mutex<HashMap<String, Sender<Result<Value, String>>>>,
}

impl PiControl {
    fn new(stdin: Arc<Mutex<Option<ChildStdin>>>, next_id: Arc<AtomicU64>) -> Self {
        Self {
            stdin,
            next_id,
            pending: Mutex::new(HashMap::new()),
        }
    }

    fn request(&self, command: &str, fields: Value) -> Result<Value, WireError> {
        let id = format!("c-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| WireError::new(ErrorCode::Io, "Pi control map is unavailable."))?
            .insert(id.clone(), tx);
        let mut frame = serde_json::json!({"id": id, "type": command});
        if let Some(object) = fields.as_object() {
            frame
                .as_object_mut()
                .expect("control frame is an object")
                .extend(object.clone());
        }
        if let Err(error) = send_json(&self.stdin, &frame, "Pi") {
            let _ = self.pending.lock().map(|mut pending| pending.remove(&id));
            return Err(error);
        }
        let response = rx.recv_timeout(RESPONSE_TIMEOUT).map_err(|error| {
            let _ = self.pending.lock().map(|mut pending| pending.remove(&id));
            WireError::new(
                ErrorCode::Io,
                format!("Pi {command} response timed out: {error}"),
            )
        })?;
        let response = response.map_err(|message| WireError::new(ErrorCode::Io, message))?;
        if response.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "Pi {command} failed: {}",
                    response
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                ),
            ));
        }
        Ok(response)
    }

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
}

struct PiSwitcher {
    control: Arc<PiControl>,
    catalog: Arc<Mutex<PiCatalog>>,
    mode_id: Arc<Mutex<String>>,
    permission_extension_active: Arc<AtomicBool>,
}

impl ModelSwitcher for PiSwitcher {
    fn set_model(&self, model_id: Option<&str>, effort: Option<&str>) -> Result<(), WireError> {
        let current = self
            .catalog
            .lock()
            .map_err(|_| WireError::new(ErrorCode::Io, "Pi model catalog is unavailable."))?
            .clone();
        if model_id.is_none() {
            if let Some(effort) = effort {
                if !thinking_level_allowed(effort, &current.current_levels) {
                    return Err(WireError::new(
                        ErrorCode::InvalidRequest,
                        format!(
                            "Pi thinking level '{effort}' is not available for the current model."
                        ),
                    ));
                }
                self.control
                    .request("set_thinking_level", serde_json::json!({"level": effort}))?;
                self.catalog
                    .lock()
                    .map_err(|_| WireError::new(ErrorCode::Io, "Pi model catalog is unavailable."))?
                    .current_effort = Some(effort.to_string());
            }
            return Ok(());
        }
        let model_id = model_id.expect("checked above");
        let model = current.models.get(model_id).ok_or_else(|| {
            WireError::new(
                ErrorCode::InvalidRequest,
                format!("Pi model '{model_id}' is not in get_available_models."),
            )
        })?;
        let provider = model.provider.clone().ok_or_else(|| {
            WireError::new(
                ErrorCode::InvalidRequest,
                format!("Pi model '{model_id}' has no provider."),
            )
        })?;
        if let Some(effort) = effort {
            if let Some(efforts) = &model.efforts {
                let available = efforts
                    .iter()
                    .map(|effort| effort.id.clone())
                    .collect::<Vec<_>>();
                if !thinking_level_allowed(effort, &available) {
                    return Err(WireError::new(
                        ErrorCode::InvalidRequest,
                        format!(
                            "Pi thinking level '{effort}' is not available for model '{model_id}'."
                        ),
                    ));
                }
            }
        }
        let model_request = self.control.request(
            "set_model",
            serde_json::json!({"provider": provider, "modelId": model_id}),
        );
        if let Err(error) = model_request {
            return Err(self.rollback_error(&current, error));
        }
        let levels_response = match self
            .control
            .request("get_available_thinking_levels", Value::Null)
        {
            Ok(response) => response,
            Err(error) => return Err(self.rollback_error(&current, error)),
        };
        let levels = levels_response
            .get("data")
            .and_then(|data| data.get("levels"))
            .and_then(Value::as_array)
            .map(|levels| {
                levels
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(effort) = effort {
            if !thinking_level_allowed(effort, &levels) {
                let error = WireError::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "Pi thinking level '{effort}' is not available for model '{model_id}'."
                    ),
                );
                return Err(self.rollback_error(&current, error));
            }
            if let Err(error) = self
                .control
                .request("set_thinking_level", serde_json::json!({"level": effort}))
            {
                return Err(self.rollback_error(&current, error));
            }
        }
        let current_effort = effort.map(str::to_string).or_else(|| {
            current
                .current_effort
                .clone()
                .filter(|effort| thinking_level_allowed(effort, &levels))
        });
        let mut catalog = match self.catalog.lock() {
            Ok(catalog) => catalog,
            Err(_) => {
                let error = WireError::new(ErrorCode::Io, "Pi model catalog is unavailable.");
                return Err(self.rollback_error(&current, error));
            }
        };
        catalog.current_model_id = Some(model_id.to_string());
        catalog.current_provider = Some(provider);
        catalog.current_levels = levels;
        catalog.current_effort = current_effort;
        Ok(())
    }

    fn set_mode(&self, mode_id: &str) -> Result<(), WireError> {
        if !matches!(mode_id, "bypass" | "ask") {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("Pi session mode '{mode_id}' is not available."),
            ));
        }
        if mode_id == "ask" && !self.permission_extension_active.load(Ordering::Acquire) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Pi permission extension not active.",
            ));
        }
        *self
            .mode_id
            .lock()
            .map_err(|_| WireError::new(ErrorCode::Io, "Pi mode state is unavailable."))? =
            mode_id.to_string();
        Ok(())
    }

    fn manifest(&self) -> Option<SessionEvent> {
        let mode_id = self.mode_id.lock().ok()?.clone();
        self.catalog
            .lock()
            .ok()
            .map(|catalog| manifest_from_catalog(&catalog, &mode_id))
    }

    fn clone_switcher(&self) -> Box<dyn ModelSwitcher> {
        Box::new(Self {
            control: Arc::clone(&self.control),
            catalog: Arc::clone(&self.catalog),
            mode_id: Arc::clone(&self.mode_id),
            permission_extension_active: Arc::clone(&self.permission_extension_active),
        })
    }
}

impl PiSwitcher {
    fn rollback_error(&self, current: &PiCatalog, error: WireError) -> WireError {
        match self.restore_model(current) {
            Ok(()) => error,
            Err(rollback) => WireError::new(
                ErrorCode::Io,
                format!(
                    "{}; Pi model rollback failed: {}",
                    error.message, rollback.message
                ),
            ),
        }
    }

    fn restore_model(&self, current: &PiCatalog) -> Result<(), WireError> {
        let provider = current.current_provider.clone().ok_or_else(|| {
            WireError::new(
                ErrorCode::Io,
                "Pi cannot roll back a model without the previous provider.",
            )
        })?;
        let model_id = current.current_model_id.clone().ok_or_else(|| {
            WireError::new(
                ErrorCode::Io,
                "Pi cannot roll back a model without the previous model.",
            )
        })?;
        self.control.request(
            "set_model",
            serde_json::json!({"provider": provider, "modelId": model_id}),
        )?;
        if let Some(effort) = &current.current_effort {
            self.control
                .request("set_thinking_level", serde_json::json!({"level": effort}))?;
        }
        Ok(())
    }
}

fn thinking_level_allowed(level: &str, available: &[String]) -> bool {
    available.iter().any(|candidate| candidate == level)
}

struct PiReader {
    buffer: Vec<u8>,
    discarding_oversized_line: bool,
    deferred: Vec<Value>,
    manifest: Option<SessionEvent>,
    permission_broker: Arc<PermissionBroker>,
    controls: Arc<Mutex<HashMap<u64, String>>>,
    next_id: Arc<AtomicU64>,
    control: Arc<PiControl>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    permission_extension_active: Arc<AtomicBool>,
    extension_path: PathBuf,
}

impl PiReader {
    #[allow(clippy::too_many_arguments)]
    fn new(
        deferred: Vec<Value>,
        manifest: SessionEvent,
        permission_broker: Arc<PermissionBroker>,
        controls: Arc<Mutex<HashMap<u64, String>>>,
        next_id: Arc<AtomicU64>,
        control: Arc<PiControl>,
        stdin: Arc<Mutex<Option<ChildStdin>>>,
        permission_extension_active: Arc<AtomicBool>,
    ) -> Self {
        Self {
            buffer: Vec::new(),
            discarding_oversized_line: false,
            deferred,
            manifest: Some(manifest),
            permission_broker,
            controls,
            next_id,
            control,
            stdin,
            permission_extension_active,
            extension_path: PathBuf::new(),
        }
    }

    fn with_extension_path(mut self, extension_path: PathBuf) -> Self {
        self.extension_path = extension_path;
        self
    }

    fn publish(&self, runtime: &SessionRuntime, event: SessionEvent, seq: Option<u64>) {
        let _ = runtime.publish_agent_event_with_seq(event, None, seq);
    }

    fn dispatch_value(
        &mut self,
        value: Value,
        runtime: &Arc<SessionRuntime>,
    ) -> Result<(), String> {
        let event_seq = runtime.journal_agent_envelope(&value);
        if value.get("type").and_then(Value::as_str) == Some("response") {
            let _ = self.control.deliver(&value);
            return Ok(());
        }
        if let Some(session_id) = session_id_from_value(&value) {
            runtime.set_peer_session_id(session_id);
        }
        if value.get("type").and_then(Value::as_str) == Some("extension_error") {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: format!("Pi permission extension error: {value}"),
                },
                event_seq,
            );
            return Ok(());
        }
        if value.get("type").and_then(Value::as_str) == Some("extension_ui_request") {
            return self
                .dispatch_ui_request(&value, runtime, event_seq)
                .map_err(|error| format!("Pi UI request failed: {error}"));
        }
        let mut event_seq = event_seq;
        for event in crate::pi_view::events_from_line(&value) {
            self.publish(runtime, event, event_seq.take());
        }
        Ok(())
    }

    fn dispatch_ui_request(
        &mut self,
        value: &Value,
        runtime: &Arc<SessionRuntime>,
        event_seq: Option<u64>,
    ) -> Result<(), String> {
        if is_ready_notify(value) {
            self.permission_extension_active
                .store(true, Ordering::Release);
            return Ok(());
        }
        let method = value
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let request_id = value.get("id").and_then(Value::as_str);
        if method != "confirm" || request_id.is_none() {
            send_extension_response(&self.stdin, request_id, false)
                .map_err(|error| format!("Could not deny Pi UI request: {error}"))?;
            return Ok(());
        }
        let request_id = request_id.expect("checked above");
        let event = permission_request_from_ui(value, request_id);
        let broker_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.controls
            .lock()
            .map_err(|_| "Pi permission map is unavailable.".to_string())?
            .insert(broker_id, request_id.to_string());
        if let Err(error) = self
            .permission_broker
            .register(broker_id, event.clone(), runtime)
        {
            self.controls
                .lock()
                .map_err(|_| "Pi permission map is unavailable.".to_string())?
                .remove(&broker_id);
            send_extension_response(&self.stdin, Some(request_id), false)
                .map_err(|send_error| format!("Could not deny Pi UI request: {send_error}"))?;
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: format!("Could not queue Pi permission request: {error}"),
                },
                event_seq,
            );
            return Ok(());
        }
        match self.permission_broker.auto_answer(request_id, runtime) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => {
                let _ = send_extension_response(&self.stdin, Some(request_id), false);
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!("Could not auto-answer Pi permission request: {error}"),
                    },
                    event_seq,
                );
                return Ok(());
            }
        }
        if runtime.permission_delivery_enabled() == Some(false) {
            self.permission_broker
                .respond(request_id, devboule_protocol::PermissionOutcome::Deny)
                .map_err(|error| format!("Could not deny Pi permission request: {error}"))?;
            return Ok(());
        }
        self.publish(runtime, event, event_seq);
        Ok(())
    }
}

fn permission_request_from_ui(value: &Value, request_id: &str) -> SessionEvent {
    let (title, description) = match value.get("title") {
        Some(Value::Object(title)) => (
            title
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Pi permission")
                .to_string(),
            title
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Allow this tool call?")
                .to_string(),
        ),
        Some(Value::String(title)) => (title.clone(), String::new()),
        _ => (
            "Pi permission".to_string(),
            "Allow this tool call?".to_string(),
        ),
    };
    let title_object = value.get("title").and_then(Value::as_object);
    let metadata_value = |name: &str| {
        title_object
            .and_then(|title| title.get(name))
            .or_else(|| value.get(name))
    };
    let command = metadata_value("command")
        .and_then(Value::as_str)
        .map(str::to_string);
    let args = metadata_value("args")
        .and_then(Value::as_array)
        .map(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        });
    let cwd = metadata_value("cwd")
        .and_then(Value::as_str)
        .map(str::to_string);
    SessionEvent::PermissionRequest {
        tool_call_id: request_id.to_string(),
        title,
        description: Some(description),
        command,
        args,
        cwd,
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
    }
}

fn send_extension_response(
    stdin: &Mutex<Option<ChildStdin>>,
    request_id: Option<&str>,
    confirmed: bool,
) -> io::Result<()> {
    let frame = serde_json::json!({
        "id": request_id,
        "type": "extension_ui_response",
        "confirmed": confirmed,
    });
    let mut bytes = serde_json::to_vec(&frame)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    write_child_stdin(stdin, &bytes, "Pi")
}

impl ReaderDispatch for PiReader {
    fn feed(&mut self, bytes: &[u8], runtime: &Arc<SessionRuntime>) -> Result<(), String> {
        if self.manifest.is_some() {
            let manifest = self.manifest.take().expect("checked above");
            let manifest = runtime.store_session_manifest(manifest);
            self.publish(runtime, manifest, None);
            for value in std::mem::take(&mut self.deferred) {
                self.dispatch_value(value, runtime)?;
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
                    self.publish(
                        runtime,
                        SessionEvent::AgentError {
                            message: format!(
                                "Pi input line exceeded {MAX_LINE_BYTES} bytes and was discarded."
                            ),
                        },
                        None,
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
                            "Pi input line exceeded {MAX_LINE_BYTES} bytes and was discarded."
                        ),
                    },
                    None,
                );
                continue;
            }
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let value = match serde_json::from_slice::<Value>(line) {
                Ok(value) => value,
                Err(error) => {
                    self.publish(
                        runtime,
                        SessionEvent::AgentError {
                            message: format!("Malformed Pi output was discarded: {error}"),
                        },
                        None,
                    );
                    continue;
                }
            };
            self.dispatch_value(value, runtime)?;
        }
        Ok(())
    }

    fn finish(&mut self, runtime: &Arc<SessionRuntime>) {
        self.permission_broker.close();
        if !self.buffer.is_empty() {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: "Pi agent ended with an unterminated output line.".to_string(),
                },
                None,
            );
        }
        remove_permission_extension(&self.extension_path);
    }
}

fn is_read_only_tool(name: &str) -> bool {
    PI_TOOL_POLICIES
        .iter()
        .any(|tool| tool.name == name && !tool.requires_confirmation)
}

struct PiStdout {
    receiver: Receiver<io::Result<Vec<u8>>>,
    buffer: Vec<u8>,
}

impl PiStdout {
    fn spawn(mut stdout: impl Read + Send + 'static) -> io::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("session-pi-stdout".to_string())
            .spawn(move || {
                let mut bytes = [0u8; 16 * 1024];
                loop {
                    match stdout.read(&mut bytes) {
                        Ok(0) => return,
                        Ok(length) => {
                            if sender.send(Ok(bytes[..length].to_vec())).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = sender.send(Err(error));
                            return;
                        }
                    }
                }
            })?;
        Ok(Self {
            receiver,
            buffer: Vec::new(),
        })
    }

    fn next_line(&mut self, deadline: Instant) -> io::Result<Option<String>> {
        loop {
            if let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
                if newline.saturating_add(1) > MAX_LINE_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Pi handshake line exceeded the 10 MiB limit",
                    ));
                }
                let line: Vec<u8> = self.buffer.drain(..=newline).collect();
                let line = line.strip_suffix(b"\n").unwrap_or(&line);
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                return Ok(Some(String::from_utf8_lossy(line).into_owned()));
            }
            if self.buffer.len() > MAX_LINE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Pi handshake line exceeded the 10 MiB limit",
                ));
            }
            let timeout = deadline.saturating_duration_since(Instant::now());
            if timeout.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Pi permission channel handshake timed out",
                ));
            }
            match self.receiver.recv_timeout(timeout) {
                Ok(Ok(bytes)) => self.buffer.extend_from_slice(&bytes),
                Ok(Err(error)) => return Err(error),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Pi permission channel handshake timed out",
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }
}

impl Read for PiStdout {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        while self.buffer.is_empty() {
            match self.receiver.recv() {
                Ok(Ok(bytes)) => self.buffer.extend_from_slice(&bytes),
                Ok(Err(error)) => return Err(error),
                Err(_) => return Ok(0),
            }
        }
        let length = output.len().min(self.buffer.len());
        output[..length].copy_from_slice(&self.buffer[..length]);
        self.buffer.drain(..length);
        Ok(length)
    }
}

struct PiStderr {
    stderr: Option<ChildStderr>,
}

impl PiStderr {
    fn start(stderr: ChildStderr) -> io::Result<Self> {
        Ok(Self {
            stderr: Some(stderr),
        })
    }
}

impl StderrSource for PiStderr {
    fn spawn(mut self: Box<Self>, runtime: Arc<SessionRuntime>) -> io::Result<JoinHandle<()>> {
        let mut stderr = self
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("Pi stderr drain was already consumed"))?;
        std::thread::Builder::new()
            .name("session-pi-stderr".to_string())
            .spawn(move || {
                let mut buffer = [0u8; 4096];
                loop {
                    match stderr.read(&mut buffer) {
                        Ok(0) => return,
                        Ok(length) => {
                            let data = String::from_utf8_lossy(&buffer[..length]).into_owned();
                            let _ = runtime
                                .publish_agent_event(SessionEvent::AgentStderr { data }, None);
                        }
                        Err(_) => return,
                    }
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        carried_pi_mime_types, is_ready_notify, perform_handshake, permission_extension_path,
        permission_request_from_ui, pi_delivery, pi_image_entry, pi_permission_sender,
        pi_prompt_frame, plan_pi_prompt, spawn_args, thinking_level_allowed,
        write_permission_extension, PiCatalog, PiControl, PiStaticPrompt, PiStdout, PiSwitcher,
    };
    use crate::acp_view::PromptCapabilityState;
    use crate::attachment_store::AttachmentStore;
    use crate::pi_view::events_from_line;
    use crate::raster_metadata::{clean_png, png_with_text_chunk, vector_input, vector_output};
    use crate::session::{ModelSwitcher, PtyCommand, ReaderDispatch, StaticImageSink};
    use devboule_protocol::{PromptAttachment, SessionEvent};
    use std::collections::HashMap;
    use std::io::BufRead;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    /// One `node` spawner for the Pi tests, so a missing binary fails the
    /// same way everywhere.
    fn node_command() -> std::process::Command {
        std::process::Command::new("node")
    }

    /// What a `node` spawn failure panics with: the test name, the program
    /// name, the `io::Error` (kind + OS message), and the `PATH` and cwd the
    /// test process saw — the context the old "node is required" message
    /// never gave.
    fn node_unavailable(test: &str, error: &std::io::Error) -> String {
        format!(
            "node is required for the {test}: could not spawn `node` ({error}; kind={:?}; PATH={:?}; cwd={:?})",
            error.kind(),
            std::env::var("PATH").unwrap_or_default(),
            std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        )
    }

    /// `get_available_models` as Pi answered it over `pi --mode rpc` (captured
    /// 2026-09-10). The capture elided each model's `cost` object and the body
    /// of `thinkingLevelMap`; those two omissions are the only difference from
    /// the wire. Per-model `input` is the part under test, and it is verbatim.
    const PI_MODELS_CAPTURE: &str = r#"{"data":{"models":[
{"id":"minimax-m3","name":"MiniMax-M3","api":"anthropic-messages","provider":"opencode-go","reasoning":true,"input":["text","image"],"contextWindow":1000000,"maxTokens":131072},
{"id":"deepseek-v4-flash","name":"DeepSeek V4 Flash","api":"openai-completions","reasoning":true,"input":["text"]},
{"id":"deepseek-v4-flash-vision-exp","name":"DeepSeek V4 Flash Vision Exp","reasoning":true,"input":["text","image"]}
]}}"#;

    fn catalog_from_capture(capture: &str) -> PiCatalog {
        let state = serde_json::json!({
            "data": {
                "sessionId": "session-1",
                "model": {"id": "minimax-m3", "provider": "opencode-go"},
            }
        });
        let models: serde_json::Value = serde_json::from_str(capture).expect("captured models");
        let levels = serde_json::json!({"data": {"levels": ["high"]}});
        super::catalog_from_responses(&state, &models, &levels).expect("catalog")
    }

    #[test]
    fn thinking_level_validation_is_against_the_current_model_list() {
        let levels = ["low".to_string(), "high".to_string(), "max".to_string()];
        assert!(!thinking_level_allowed("panzeroni", &levels));
        assert!(!thinking_level_allowed("xhigh", &levels));
        assert!(thinking_level_allowed("high", &levels));
    }

    #[test]
    fn ask_mode_is_rejected_until_the_permission_extension_reports_in() {
        let active = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let switcher = PiSwitcher {
            control: Arc::new(PiControl::new(
                Arc::new(Mutex::new(None)),
                Arc::new(std::sync::atomic::AtomicU64::new(1)),
            )),
            catalog: Arc::new(Mutex::new(PiCatalog::default())),
            mode_id: Arc::new(Mutex::new("bypass".to_string())),
            permission_extension_active: Arc::clone(&active),
        };
        let error = switcher.set_mode("ask").expect_err("extension is inactive");
        assert_eq!(error.message, "Pi permission extension not active.");
        switcher.set_mode("bypass").expect("bypass is immediate");
        active.store(true, Ordering::Release);
        switcher.set_mode("ask").expect("extension is active");
    }

    #[test]
    fn only_our_notify_is_the_permission_channel_ready_signal() {
        let ready = serde_json::json!({
            "type": "extension_ui_request",
            "method": "notify",
            "message": "devboule-permission-channel"
        });
        let session = serde_json::json!({"type": "session", "id": "session-1"});
        assert!(is_ready_notify(&ready));
        assert!(!is_ready_notify(&session));
    }

    #[test]
    fn handshake_does_not_require_the_ready_signal() {
        let mut child = node_command()
            .args(["-e", "setTimeout(() => {}, 10000)"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("{}", node_unavailable("Pi handshake test", &error)));
        let stdin = child.stdin.take().expect("node stdin");
        let stdin = std::sync::Mutex::new(Some(stdin));
        let mut stdout = PiStdout::spawn(std::io::Cursor::new(
            br#"{"id":"h-1","type":"response","success":true,"data":{"sessionId":"session-1","model":{"id":"m","provider":"p"}}}
{"id":"h-2","type":"response","success":true,"data":{"models":[{"id":"m","name":"M","provider":"p"}]}}
{"id":"h-3","type":"response","success":true,"data":{"levels":["medium"]}}
"#
            .to_vec(),
        ))
        .expect("stdout reader");
        let result = perform_handshake(
            &mut stdout,
            &stdin,
            &std::sync::atomic::AtomicU64::new(1),
            "bypass",
        );
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            result.is_ok(),
            "handshake must not wait for the extension notify: {:?}",
            result.err()
        );
    }

    #[test]
    fn spawn_args_keep_rpc_and_add_our_extension() {
        let command = PtyCommand::new(
            "pi",
            Vec::new(),
            std::env::current_dir().expect("cwd"),
            Vec::new(),
        );
        let path = Path::new(r"C:\runtime\devboule-pi-permissions.ts");
        let args = spawn_args(&command, path).expect("Pi args");
        let mode = args.iter().position(|arg| arg == "--mode").expect("mode");
        let extension = args.iter().position(|arg| arg == "-e").expect("extension");
        assert!(mode < extension);
        assert_eq!(args[mode + 1], "rpc");
        assert_eq!(args[extension + 1], path.to_string_lossy());
        assert!(!args.iter().any(|arg| arg == "--no-extensions"));
        assert!(!args.iter().any(|arg| arg == "--tools"));
    }

    #[test]
    fn caller_extensions_are_preserved_alongside_ours() {
        let command = PtyCommand::new(
            "pi",
            vec![
                "--extension".to_string(),
                "first.ts".to_string(),
                "--extension=second.ts".to_string(),
            ],
            std::env::current_dir().expect("cwd"),
            Vec::new(),
        );
        let path = Path::new("permission.ts");
        let args = spawn_args(&command, path).expect("caller extensions are valid");
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--extension".to_string(), "first.ts".to_string()]));
        assert!(args.iter().any(|arg| arg == "--extension=second.ts"));
        assert_eq!(args.iter().filter(|arg| *arg == "-e").count(), 1);
        let path = path.to_string_lossy().into_owned();
        assert!(args.iter().any(|arg| arg == &path));
    }

    #[test]
    fn only_non_rpc_pi_modes_are_rejected() {
        for supplied in [
            vec!["--mode"],
            vec!["--mode", "interactive"],
            vec!["--mode=interactive"],
        ] {
            let command = PtyCommand::new(
                "pi",
                supplied.iter().map(|arg| (*arg).to_string()).collect(),
                std::env::current_dir().expect("cwd"),
                Vec::new(),
            );
            spawn_args(&command, Path::new("permission.ts"))
                .expect_err("non-rpc mode must be rejected");
        }
        for supplied in [
            vec!["--mode", "rpc"],
            vec!["--mode=rpc"],
            vec!["--extension", "evil.ts"],
            vec!["-e=evil.ts"],
        ] {
            let command = PtyCommand::new(
                "pi",
                supplied.iter().map(|arg| (*arg).to_string()).collect(),
                std::env::current_dir().expect("cwd"),
                Vec::new(),
            );
            spawn_args(&command, Path::new("permission.ts"))
                .expect("rpc mode and caller extensions are valid");
        }
    }

    #[test]
    fn wire_shaped_pi_confirm_keeps_permission_command_metadata() {
        let request = permission_request_from_ui(
            &serde_json::json!({
                "type": "extension_ui_request",
                "method": "confirm",
                "id": "ui-1",
                "title": {
                    "title": "Devboule permission",
                    "message": "Allow bash?",
                    "command": "bash",
                    "args": ["command=pi", "-p", "touch outside.txt"],
                    "cwd": "C:/workspace"
                }
            }),
            "ui-1",
        );
        assert!(matches!(
            request,
            SessionEvent::PermissionRequest {
                command: Some(command),
                args: Some(args),
                cwd: Some(cwd),
                ..
            } if command == "bash"
                && args
                    == vec![
                        "command=pi".to_string(),
                        "-p".to_string(),
                        "touch outside.txt".to_string(),
                    ]
                && cwd == "C:/workspace"
        ));
    }

    #[test]
    fn pi_permission_sender_keeps_control_after_failed_write() {
        let stdin: Arc<Mutex<Option<std::process::ChildStdin>>> = Arc::new(Mutex::new(None));
        let controls = Arc::new(Mutex::new(HashMap::from([(7, "ui-7".to_string())])));
        let sender = pi_permission_sender(Arc::clone(&stdin), Arc::clone(&controls));

        assert!((sender)(7, serde_json::json!({"outcome": {"outcome": "cancelled"}})).is_err());
        assert!(controls.lock().expect("controls").contains_key(&7));
    }

    #[test]
    fn pi_auto_answer_failure_still_denies_the_extension_confirm() {
        let mut child = node_command()
            .args([
                "-e",
                "process.stdin.on('data', data => process.stdout.write(data))",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("{}", node_unavailable("Pi auto-answer test", &error)));
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let mut stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
        let broker = super::PermissionBroker::for_test(Arc::new(|_, _| {
            Err(std::io::Error::other("synthetic completion failure"))
        }));
        let mut reader = super::PiReader::new(
            Vec::new(),
            SessionEvent::SessionManifest {
                provider_id: Some("pi".to_string()),
                current_model_id: None,
                models: Vec::new(),
                modes: Some(devboule_protocol::SessionModeStateView {
                    current_mode_id: "bypass".to_string(),
                    available_modes: Vec::new(),
                }),
            },
            Arc::clone(&broker),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(std::sync::atomic::AtomicU64::new(1)),
            Arc::new(super::PiControl::new(
                Arc::clone(&stdin),
                Arc::new(std::sync::atomic::AtomicU64::new(1)),
            )),
            Arc::clone(&stdin),
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
        );
        let runtime = Arc::new(crate::session::SessionRuntime::new());
        let request = serde_json::json!({
            "type": "extension_ui_request",
            "method": "confirm",
            "id": "ui-1",
            "title": {"title": "Pi permission", "message": "Allow bash?"},
            "command": "bash"
        });
        reader
            .feed(format!("{request}\n").as_bytes(), &runtime)
            .expect("confirm request");
        let mut line = String::new();
        stdout
            .read_line(&mut line)
            .expect("definite extension response");
        let response: serde_json::Value = serde_json::from_str(&line).expect("response json");
        assert_eq!(response["type"], "extension_ui_response");
        assert_eq!(response["id"], "ui-1");
        assert_eq!(response["confirmed"], false);
        assert_eq!(broker.pending_len(), 0);
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn permission_extension_paths_are_unique_per_session() {
        let runtime = Path::new(r"C:\runtime");
        assert_ne!(
            permission_extension_path(runtime),
            permission_extension_path(runtime)
        );
    }

    #[test]
    fn permission_extension_prompts_unknown_tools_and_allows_confirmed_tools() {
        let path = std::env::temp_dir().join(format!(
            "devboule-pi-permission-test-{}.mjs",
            std::process::id()
        ));
        write_permission_extension(&path).expect("permission extension");
        let script = r#"
(async () => {
  const { pathToFileURL } = require("url");
  const extension = await import(pathToFileURL(process.argv[1]).href);
  const handlers = {};
  extension.default({ on(name, handler) { handlers[name] = handler; } });
  const confirmations = [];
  const context = {
    cwd: "C:/workspace",
    ui: {
      notify() {},
      async confirm(request) {
        const confirmed = request.command !== "tool-futuro";
        confirmations.push({ request, confirmed });
        return confirmed;
      },
    },
  };
  for (const toolName of ["read", "grep", "find", "ls"]) {
    const result = await handlers.tool_call({ toolName, input: {} }, context);
    if (result !== undefined) process.exit(4);
  }
  const allowed = await handlers.tool_call({
    toolName: "write",
    input: { command: "pi -p touch outside.txt" },
  }, context);
  const allowedBash = await handlers.tool_call({
    toolName: "bash",
    input: { command: "pi -p touch outside.txt" },
  }, context);
  const denied = await handlers.tool_call({
    toolName: "tool-futuro",
    input: {},
  }, context);
  if (allowed !== undefined || allowedBash !== undefined || denied?.block !== true) process.exit(1);
  if (confirmations.length !== 3
      || confirmations[0].request.command !== "write"
      || confirmations[0].confirmed !== true
      || confirmations[1].request.command !== "bash"
      || confirmations[1].confirmed !== true
      || confirmations[0].request.args[0] !== "command=pi -p touch outside.txt"
      || confirmations[2].request.command !== "tool-futuro"
      || confirmations[2].confirmed !== false) {
    process.exit(2);
  }
})().catch((error) => { console.error(error); process.exit(3); });
"#;
        let output = node_command()
            .args(["-e", script, &path.to_string_lossy()])
            .output()
            .unwrap_or_else(|error| {
                panic!("{}", node_unavailable("Pi gate behavior test", &error))
            });
        let _ = std::fs::remove_file(&path);
        assert!(
            output.status.success(),
            "Pi permission extension behavior failed (exit={}): {}{}",
            output
                .status
                .code()
                .map_or_else(|| "no exit code".to_string(), |code| code.to_string()),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn write_permission_extension_requires_the_runtime_parent() {
        let path = std::env::temp_dir()
            .join("devboule-pi-missing-parent")
            .join("permission.ts");
        let error = write_permission_extension(&path).expect_err("missing parent must fail");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn exact_recorded_text_and_turn_lines_translate_without_message_end_text() {
        let text = serde_json::from_str::<serde_json::Value>(r#"{"type":"message_update","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"OK"}}"#).expect("recording");
        let text_end = serde_json::from_str::<serde_json::Value>(r#"{"type":"message_update","usage":{"input":25848,"output":3,"cacheRead":0,"cacheWrite":0,"reasoning":0,"totalTokens":25851,"cost":{"input":0.0019386,"output":7.5e-7,"cacheRead":0,"cacheWrite":0,"total":0.00193935}},"assistantMessageEvent":{"type":"text_end","contentIndex":0,"content":"OK"}}"#).expect("recording");
        assert!(
            matches!(events_from_line(&text).as_slice(), [SessionEvent::AgentMessage { text, .. }] if text == "OK")
        );
        assert!(events_from_line(&text_end).is_empty());
    }

    #[test]
    fn captured_models_keep_image_support_apart_per_model() {
        // The real reply: two models declare images and one does not, so the
        // answer is a property of the model, not of the session.
        let catalog = catalog_from_capture(PI_MODELS_CAPTURE);

        let minimax = catalog.input_kinds("minimax-m3").expect("minimax-m3");
        assert_eq!(minimax.image, PromptCapabilityState::Supported);
        assert_eq!(minimax.declared, vec!["text", "image"]);

        let flash = catalog
            .input_kinds("deepseek-v4-flash")
            .expect("deepseek-v4-flash");
        assert_eq!(flash.image, PromptCapabilityState::Unsupported);
        assert_eq!(flash.declared, vec!["text"]);

        let vision = catalog
            .input_kinds("deepseek-v4-flash-vision-exp")
            .expect("deepseek-v4-flash-vision-exp");
        assert_eq!(vision.image, PromptCapabilityState::Supported);
    }

    #[test]
    fn input_array_keeps_kinds_we_do_not_model() {
        // A future kind must be kept, not dropped and not treated as an error.
        let capture = r#"{"data":{"models":[
{"id":"a","name":"A","provider":"p","input":["text","image","audio","video","hologram"]},
{"id":"b","name":"B","provider":"p","input":["text","audio"]}
]}}"#;
        let catalog = catalog_from_capture(capture);

        let a = catalog.input_kinds("a").expect("model a");
        assert_eq!(
            a.declared,
            vec!["text", "image", "audio", "video", "hologram"],
            "unknown kinds must survive verbatim"
        );
        assert_eq!(a.image, PromptCapabilityState::Supported);

        let b = catalog.input_kinds("b").expect("model b");
        assert_eq!(b.declared, vec!["text", "audio"]);
        assert_eq!(b.image, PromptCapabilityState::Unsupported);
    }

    #[test]
    fn absent_or_malformed_input_is_not_a_refusal() {
        let capture = r#"{"data":{"models":[
{"id":"silent","name":"Silent","provider":"p"},
{"id":"malformed","name":"Malformed","provider":"p","input":"text"},
{"id":"empty","name":"Empty","provider":"p","input":[]}
]}}"#;
        let catalog = catalog_from_capture(capture);

        for model_id in ["silent", "malformed"] {
            let kinds = catalog.input_kinds(model_id).expect(model_id);
            assert_eq!(
                kinds.image,
                PromptCapabilityState::Absent,
                "{model_id} never declared anything; silence is not a refusal"
            );
            assert!(kinds.declared.is_empty());
        }

        // A present array is a declaration even when it is empty: it lists no
        // image, and that is an answer rather than silence.
        let empty = catalog.input_kinds("empty").expect("empty");
        assert_eq!(empty.image, PromptCapabilityState::Unsupported);
        assert!(empty.declared.is_empty());
    }

    // --- image delivery (the static route) --------------------------------
    //
    // The routing decision lives in `plan_pi_prompt`, tested here against
    // the attachment store directly, without spawning a child — the same
    // arrangement the ACP sibling seam's tests use. The wire shape of one
    // entry is pinned against Paseo's measured `convertPromptInput` output
    // (`{"type":"image","data":...,"mimeType":"image/png"}`), and the
    // frame omission against `...(images?.length ? { images } : {})`.

    struct PlanTempDir(std::path::PathBuf);

    impl PlanTempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let dir = std::env::temp_dir().join(format!(
                "devboule-pi-plan-{}-{}-{}",
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

    fn capable_catalog() -> PiCatalog {
        catalog_from_capture(PI_MODELS_CAPTURE)
    }

    #[test]
    fn pi_delivery_follows_the_current_model_tri_state() {
        // Paseo's `piModelSupportsImageInput` through the tri-state this
        // daemon already keeps: `Supported` frames bytes, `Unsupported` AND
        // `Absent` keep the path line.
        let catalog = capable_catalog();
        assert_eq!(
            pi_delivery(&catalog, Some("minimax-m3")),
            super::super::ImageDelivery::StaticImageBlock,
            "a model that declared image frames bytes"
        );
        assert_eq!(
            pi_delivery(&catalog, Some("deepseek-v4-flash")),
            super::super::ImageDelivery::PathLine,
            "a model that declared no image keeps the path line"
        );
        assert_eq!(
            pi_delivery(&catalog, Some("no-such-model")),
            super::super::ImageDelivery::PathLine,
            "an unknown model is Absent: never an attempt"
        );
        assert_eq!(
            pi_delivery(&catalog, None),
            super::super::ImageDelivery::PathLine,
            "no current model is Absent: never an attempt"
        );
    }

    #[test]
    fn a_capable_pi_model_plans_an_image_entry_and_no_path_line() {
        // The raster becomes one `images[]` entry; the text is the bare user
        // text, with no path line. The frame carries the field; the
        // text-only frame omits it.
        let temp = PlanTempDir::new("capable");
        let store = AttachmentStore::new(&temp.0);
        let catalog = capable_catalog();
        // A container the walk accepts but changes: what the entry carries
        // must be the stripped bytes, never the wire bytes.
        let sent = png_with_text_chunk();
        let kept = clean_png(0x01);
        assert_ne!(
            sent, kept,
            "the fixture must actually carry something that leaves"
        );
        let plan = plan_pi_prompt(
            &store,
            "pi-plan-capable",
            "describe this",
            &[plan_attachment("photo.png", "image/png", &sent)],
            &catalog,
            Some("minimax-m3"),
        )
        .expect("materialized")
        .expect("a capable model plans an entry");
        assert_eq!(plan.fallback_text, "describe this", "no path line");
        assert_eq!(carried_pi_mime_types(Some(&plan)), vec!["image/png"]);
        assert_eq!(plan.images.len(), 1);
        {
            use base64::Engine;
            assert_eq!(
                plan.images[0].data_base64,
                base64::engine::general_purpose::STANDARD.encode(&kept),
                "the entry carries the stripped bytes"
            );
        }
        // The exact entry shape, pinned literally: flat, capital-T
        // `mimeType` — not Claude's nested `source`/`media_type`.
        let entry = pi_image_entry(&plan.images[0].mime_type, &plan.images[0].data_base64);
        assert_eq!(entry["type"], "image");
        assert_eq!(entry["mimeType"], "image/png");
        assert!(entry["data"].as_str().is_some());
        assert!(entry.get("source").is_none(), "no Claude nesting");
        assert!(entry.get("media_type").is_none(), "no Claude key");
        let frame = pi_prompt_frame("p-1", &plan.fallback_text, &plan.images);
        assert_eq!(frame["message"], "describe this");
        let images = frame["images"].as_array().expect("images array");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0]["mimeType"], "image/png");
        let bare = pi_prompt_frame("p-test", "describe this", &[]);
        assert_eq!(bare["message"], "describe this");
        assert!(
            bare.get("images").is_none(),
            "text-only omits the field, never an empty array"
        );
    }

    #[test]
    fn an_incapable_pi_model_keeps_the_path_line_and_plans_nothing() {
        // `deepseek-v4-flash` declares text only: the safe answer is no plan,
        // so the caller takes the legacy path-line write.
        let temp = PlanTempDir::new("incapable");
        let store = AttachmentStore::new(&temp.0);
        let catalog = capable_catalog();
        let plan = plan_pi_prompt(
            &store,
            "pi-plan-incapable",
            "describe this",
            &[plan_attachment("photo.png", "image/png", &clean_png(0x11))],
            &catalog,
            Some("deepseek-v4-flash"),
        )
        .expect("materialized");
        assert!(plan.is_none(), "an incapable model plans nothing");
    }

    #[test]
    fn an_unknown_pi_model_keeps_the_path_line_and_plans_nothing() {
        // Absent (unknown model, or no current model): silence is not
        // consent, so the path line is the safe answer. Unknown never means
        // yes — this is the third state the tri-state exists to keep apart
        // from a refusal.
        let temp = PlanTempDir::new("unknown");
        let store = AttachmentStore::new(&temp.0);
        let catalog = capable_catalog();
        for model in [Some("no-such-model"), None] {
            let plan = plan_pi_prompt(
                &store,
                "pi-plan-unknown",
                "describe this",
                &[plan_attachment("photo.png", "image/png", &clean_png(0x12))],
                &catalog,
                model,
            )
            .expect("materialized");
            assert!(plan.is_none(), "an unknown model plans nothing");
        }
    }

    #[test]
    fn an_svg_keeps_its_path_line_beside_pi_image_entries() {
        // A mixed prompt carries both: the raster as an entry, the SVG as a
        // path line in the text.
        let temp = PlanTempDir::new("mixed");
        let store = AttachmentStore::new(&temp.0);
        let catalog = capable_catalog();
        let source = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        let plan = plan_pi_prompt(
            &store,
            "pi-plan-mixed",
            "logo and photo",
            &[
                plan_attachment("photo.png", "image/png", &clean_png(0x13)),
                plan_attachment("drawing.svg", "image/svg+xml", source),
            ],
            &catalog,
            Some("minimax-m3"),
        )
        .expect("materialized")
        .expect("the raster plans an entry");
        assert_eq!(carried_pi_mime_types(Some(&plan)), vec!["image/png"]);
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
        let frame = pi_prompt_frame("p-2", &plan.fallback_text, &plan.images);
        assert!(frame["message"].as_str().expect("text").ends_with(".svg]"));
        assert_eq!(frame["images"].as_array().expect("array").len(), 1);
    }

    #[test]
    fn a_frame_without_entries_is_the_text_only_frame() {
        // The static route frames every prompt it plans through this builder,
        // including one whose entries are all path lines. With no entry it has
        // to be the frame the writer builds inline, byte for byte.
        assert_eq!(
            pi_prompt_frame("p-test", "describe this", &[]),
            serde_json::json!({"id": "p-test", "type": "prompt", "message": "describe this"})
        );
    }

    #[test]
    fn the_static_route_answers_for_the_model_current_at_prompt_time() {
        // The route reads the live catalog rather than a copy taken at spawn:
        // a model switched since then must not be answered for with the inputs
        // the old model declared. Both directions are pinned here.
        let temp = PlanTempDir::new("route-model");
        let store = AttachmentStore::new(&temp.0);
        let catalog = Arc::new(Mutex::new(capable_catalog()));
        let route = PiStaticPrompt::new(
            Arc::new(Mutex::new(None)),
            Arc::new(AtomicU64::new(1)),
            Arc::clone(&catalog),
        );
        let attachment = plan_attachment("photo.png", "image/png", &clean_png(0x41));
        catalog.lock().expect("catalog").current_model_id = Some("minimax-m3".to_string());
        let planned = route
            .plan_prompt(
                &store,
                "pi-route",
                "describe this",
                std::slice::from_ref(&attachment),
            )
            .expect("planned")
            .expect("a model that declared image plans a frame");
        assert_eq!(planned.text(), "describe this", "no path line");
        // The same route on a text-only model declines, and the caller takes
        // the legacy write.
        catalog.lock().expect("catalog").current_model_id = Some("deepseek-v4-flash".to_string());
        assert!(route
            .plan_prompt(
                &store,
                "pi-route",
                "describe this",
                std::slice::from_ref(&attachment)
            )
            .expect("planned")
            .is_none());
    }

    #[test]
    fn a_jpeg_stays_a_jpeg_in_the_pi_entry() {
        // The label `materialize` checked against the sniffed container is
        // the label the entry carries.
        const EXIF_JPEG_VECTOR: &str =
            "a jpeg whose APP1 holds EXIF, between a kept JFIF APP0 and a kept ICC APP2";
        let temp = PlanTempDir::new("jpeg");
        let store = AttachmentStore::new(&temp.0);
        let catalog = capable_catalog();
        let sent = vector_input(EXIF_JPEG_VECTOR);
        let kept = vector_output(EXIF_JPEG_VECTOR);
        assert_ne!(sent, kept, "the vector must actually strip something");
        let plan = plan_pi_prompt(
            &store,
            "pi-plan-jpeg",
            "describe this",
            &[plan_attachment("photo.jpg", "image/jpeg", &sent)],
            &catalog,
            Some("minimax-m3"),
        )
        .expect("materialized")
        .expect("a JPEG plans an entry");
        assert_eq!(plan.fallback_text, "describe this");
        assert_eq!(carried_pi_mime_types(Some(&plan)), vec!["image/jpeg"]);
        {
            use base64::Engine;
            assert_eq!(
                plan.images[0].data_base64,
                base64::engine::general_purpose::STANDARD.encode(&kept),
                "stripped JPEG bytes, JPEG label"
            );
        }
    }
}
