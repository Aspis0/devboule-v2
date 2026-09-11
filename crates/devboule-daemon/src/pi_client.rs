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
    Ok(SpawnedSession {
        process_job,
        master: None,
        killer: Box::new(killer),
        switcher: Some(Box::new(PiSwitcher {
            control,
            catalog: Arc::new(Mutex::new(handshake.catalog)),
            mode_id: Arc::new(Mutex::new(mode_id.to_string())),
            permission_extension_active,
        })),
        child: Box::new(StdioWaitableChild { process }),
        writer: Arc::new(Mutex::new(Box::new(writer) as Box<dyn Write + Send>)),
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

#[derive(Clone, Debug)]
struct PiModel {
    name: String,
    provider: Option<String>,
    context_tokens: Option<u64>,
    efforts: Option<Vec<SessionModelEffort>>,
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
        is_ready_notify, perform_handshake, permission_extension_path, permission_request_from_ui,
        pi_permission_sender, spawn_args, thinking_level_allowed, write_permission_extension,
        PiCatalog, PiControl, PiStdout, PiSwitcher,
    };
    use crate::pi_view::events_from_line;
    use crate::session::{ModelSwitcher, PtyCommand, ReaderDispatch};
    use devboule_protocol::SessionEvent;
    use std::collections::HashMap;
    use std::io::BufRead;
    use std::path::Path;
    use std::sync::atomic::Ordering;
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
}
