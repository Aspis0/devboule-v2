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
    SessionModelEffort, UnattendedState, WireError,
};
use serde_json::Value;

use super::permission_broker::{PermissionBroker, PermissionSender};
use super::PtyCommand;
use super::{
    write_child_stdin, ModelSwitcher, ReaderDispatch, SessionKiller, SessionRuntime,
    SessionSteerer, SpawnedSession, StderrSource, StdioWaitableChild, TurnToken,
};
use crate::acp_view::PromptCapabilityState;
use crate::atomic::atomic_write;
use crate::paths::RuntimePaths;
use crate::process_tree::{JobObject, ProcessHandle};
use crate::profile_delivery::ProfileDelivery;
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
    // The broker tools are daemon-decided (S3): the set is not "read only" —
    // `devboule_send_message` and `devboule_create_agent` are not reads — so
    // the rendered name states the fact (`unmediated`). Each joins with its
    // own reason; the names come from the catalog constants so a rename
    // breaks the build instead of silently unmediating nothing.
    // Roster: a read of the caller's own bearer roster (the bearer is the
    // identity, never a tool argument).
    PiToolPolicy {
        name: crate::provider_catalog::MCP_ROSTER_TOOL,
        requires_confirmation: false,
    },
    // Profile list: the ticked subset the human enabled for agents; without it
    // `devboule_create_agent` (which names a profile and nothing else) is
    // undiscoverable.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_LIST_PROFILES_TOOL,
        requires_confirmation: false,
    },
    // Send: a routed message into a live session, not a local mutation; the
    // broker judges peer callers at the origin door before anything is touched.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_SEND_MESSAGE_TOOL,
        requires_confirmation: false,
    },
    // Create: consented by the broker's own card (`creation_card`), which carries
    // the profile facts; a generic confirm here would be a second card with none
    // of them.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_CREATE_AGENT_TOOL,
        requires_confirmation: false,
    },
    // Move: applies a ticked profile to one of the caller's own live children;
    // authority is the `created_by` link plus the tick, both broker-checked.
    // A generic confirm would re-ask without those facts.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL,
        requires_confirmation: false,
    },
    // Answer: answers one pending card of one of the caller's own children under
    // the delegation switch; the broker enforces one-shot ownership and the human
    // still sees the card either way.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL,
        requires_confirmation: false,
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
    const unmediated = new Set(__UNMEDIATED_TOOLS__);
    if (unmediated.has(event.toolName)) return;
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
    let unmediated = PI_TOOL_POLICIES
        .iter()
        .filter(|tool| is_unmediated_tool(tool.name))
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    let unmediated = serde_json::to_string(&unmediated).expect("Pi tool policy is serializable");
    PERMISSION_EXTENSION_TEMPLATE.replace("__UNMEDIATED_TOOLS__", &unmediated)
}

static BRIDGE_EXTENSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// The pi MCP bridge (S5): our own extension, ~130 lines TypeScript, sibling of
/// `PERMISSION_EXTENSION_TEMPLATE`. First-class tools, not a proxy: one
/// `pi.registerTool` per broker tool (six today), closed schemas matching the
/// broker's `tools/list` documents, descriptions verbatim from
/// `provider_catalog::MCP_BROKER_TOOLS` (pinned by the S5 walking test, so a
/// catalog edit without a bridge edit fails).
///
/// Measured against pi 0.85.1 (spike): two `-e` load with permission-first order,
/// `registerTool` round-trips stub bytes to the model, the stub logs
/// `Authorization: Bearer <env token>` on `initialize` + `tools/call`, argv is
/// token-free. Identity by environment (`DEVBOULE_MCP_URL`/`DEVBOULE_MCP_TOKEN`),
/// never argv. The bridge announces `devboule-mcp-bridge` on `session_start`
/// exactly like the permission channel; when the session is expected to host
/// tools the announce is required independent of ask/bypass (absence =
/// `Unverified` in S8, detected not discovered — no rpc tool enumeration exists).
///
/// Fetch hygiene (all four, RECON + spike): string bodies (undici sends
/// `Content-Length`, never chunked — the broker refuses chunked); dual
/// `Accept: application/json, text/event-stream` (takes the broker's JSON branch);
/// `result`/`error` parsed, never HTTP status (RPC errors ride HTTP 200); `202`
/// with an empty body is success without a result (never parsed, never failed).
/// Timeout + error text (spike S3b/S3a): every MCP fetch races a named
/// `AbortSignal.timeout`, and failures re-throw with the broker URL + cause,
/// never bare `fetch failed`.
const BRIDGE_EXTENSION_TEMPLATE: &str = r#"import { Type } from "typebox";

// The Devboule MCP bridge: the daemon's broker tools as first-class pi tools.
// Identity by environment, never argv. Timeouts and error text are load-bearing:
// a wedged broker must end the tool (never hang the turn) with a sentence that
// names the broker URL and the cause.
const MCP_URL = process.env.DEVBOULE_MCP_URL ?? "";
const MCP_TOKEN = process.env.DEVBOULE_MCP_TOKEN ?? "";
const MCP_TIMEOUT_MS = 30000;

let nextRequestId = 1;

function bridgeError(method, cause) {
  const detail = cause instanceof Error ? cause.message : String(cause);
  return new Error(`devboule broker unreachable at ${MCP_URL || "<no broker url>"}: ${method}: ${detail}`);
}

function withTimeout(signal) {
  const bound = AbortSignal.timeout(MCP_TIMEOUT_MS);
  return signal ? AbortSignal.any([signal, bound]) : bound;
}

async function mcpRequest(method, params, signal) {
  const body = JSON.stringify({
    jsonrpc: "2.0",
    id: nextRequestId++,
    method,
    ...(params === undefined ? {} : { params }),
  });
  let response;
  try {
    response = await fetch(MCP_URL, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json, text/event-stream",
        Authorization: `Bearer ${MCP_TOKEN}`,
      },
      body,
      signal: withTimeout(signal),
    });
  } catch (cause) {
    throw bridgeError(method, cause);
  }
  const text = await response.text();
  if (!response.ok) {
    throw bridgeError(method, `HTTP ${response.status}: ${text.slice(0, 300)}`);
  }
  // 202 with an empty body is success with no result: do not parse, do not fail.
  if (response.status === 202 && text.length === 0) return undefined;
  let payload;
  try {
    payload = JSON.parse(text);
  } catch {
    // Or an SSE stream: take the first data: line that parses as our reply.
    for (const line of text.split("\n")) {
      if (line.startsWith("data:")) {
        try {
          payload = JSON.parse(line.slice(5).trim());
          break;
        } catch {
          /* keep scanning */
        }
      }
    }
  }
  // RPC errors ride HTTP 200: parse result/error, never the status.
  if (payload && payload.error) {
    throw new Error(`MCP ${method} error ${payload.error.code}: ${payload.error.message}`);
  }
  return payload?.result;
}

async function mcpNotify(method, params, signal) {
  const body = JSON.stringify({
    jsonrpc: "2.0",
    id: nextRequestId++,
    method,
    ...(params === undefined ? {} : { params }),
  });
  try {
    await fetch(MCP_URL, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json, text/event-stream",
        Authorization: `Bearer ${MCP_TOKEN}`,
      },
      body,
      signal: withTimeout(signal),
    });
  } catch (cause) {
    throw bridgeError(method, cause);
  }
}

async function brokerSession(signal) {
  await mcpRequest("initialize", {
    protocolVersion: "2025-06-18",
    capabilities: {},
    clientInfo: { name: "devboule-pi-bridge", version: "1" },
  }, signal);
  await mcpNotify("notifications/initialized", {}, signal);
}

export default function (pi) {
  pi.on("session_start", async (_event, ctx) => {
    ctx.ui.notify("devboule-mcp-bridge", "info");
    // In-band proof for the daemon's verification (S8): an authenticated
    // tools/list from this child's bearer is what flips it Hosted broker-side.
    // Best-effort — a failure here breaks nothing; the announce already fired
    // and the state stays Unverified until a later list lands.
    try {
      await mcpRequest("tools/list", {}, undefined);
    } catch (_ignored) {
      /* verification stays Unverified */
    }
  });

  pi.registerTool({
    name: "devboule_list_agents",
    label: "List Devboule agents",
    description: `Lists live Devboule agent sessions known by the daemon, with their display name, the session that created them, their lifecycle state and their creation depth.`,
    parameters: Type.Object({}, { additionalProperties: false }),
    async execute(_toolCallId, _params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_list_agents", arguments: {} }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_list_profiles",
    label: "List Devboule profiles",
    description: `Lists the agent profiles the human enabled for agents, in the human's own order, with the note that says when to use each one. Call this before devboule_create_agent. Each profile's unattended field is a prediction: "yes" means a session created from it approves its own permission prompts, "no" means it asks the human, "unknown" means Devboule cannot promise either way - the child may stop on its first permission card.`,
    parameters: Type.Object({}, { additionalProperties: false }),
    async execute(_toolCallId, _params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_list_profiles", arguments: {} }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_send_message",
    label: "Send Devboule message",
    description: `Sends a message to one live Devboule agent session.`,
    parameters: Type.Object(
      {
        to_agent: Type.String(),
        text: Type.String(),
      },
      { required: ["to_agent", "text"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_send_message", arguments: params }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_create_agent",
    label: "Create Devboule agent",
    description: `Creates a new Devboule agent session from a profile the human enabled for agents, and sends it an initial prompt. The human is asked to authorize the first creation from this session; the result is the new session's id, its A2A task and context, and its display name.`,
    parameters: Type.Object(
      {
        profile: Type.String({ description: "Name of a profile the human enabled for agents; see devboule_list_profiles." }),
        title: Type.String({ description: "The child's display name, 1 to 60 characters." }),
        labels: Type.Optional(Type.Record(Type.String(), Type.String(), { description: "Optional labels for the child: string to string." })),
        workspaceId: Type.Optional(Type.String()),
        cwd: Type.Optional(Type.String()),
        initialPrompt: Type.String({ description: "The child's first prompt, at most 32 KiB." }),
        notifyOnFinish: Type.Optional(Type.Boolean()),
      },
      { required: ["profile", "title", "initialPrompt"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_create_agent", arguments: params }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_set_agent_profile",
    label: "Move Devboule agent onto profile",
    description: `Moves one of your own live child sessions onto a profile the human enabled for agents: the child is asked to switch to the profile's mode, then to the profile's model and thinking option, and the profile is recorded on the child. The human is never asked, and the child is never restarted; a provider that refuses the switch refuses the move. Moving onto a profile that runs unattended is permanent - the child's row keeps the marker even if it is moved back.`,
    parameters: Type.Object(
      {
        session: Type.String({ description: "The id or display name of one of your own live child sessions." }),
        profile: Type.String({ description: "Name of a profile the human enabled for agents; see devboule_list_profiles." }),
      },
      { required: ["session", "profile"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_set_agent_profile", arguments: params }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_answer_permission",
    label: "Answer Devboule permission",
    description: `Answers one pending permission card of one of your own live children, when the human has turned permission delegation on. The card reaches you as an agent_permission_request notice naming its cardId. outcome is allow_once or deny - never anything durable, and never a card that is not your child's. The human still sees the card either way.`,
    parameters: Type.Object(
      {
        cardId: Type.String(),
        outcome: Type.Union([Type.Literal("allow_once"), Type.Literal("deny")]),
      },
      { required: ["cardId", "outcome"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_answer_permission", arguments: params }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });
}
"#;

/// The bridge source (S5 test hook): static today — identity travels by env at
/// runtime, so every session's bytes are identical and only the file name is
/// per-session. A single function so the S5 walking test drives the exact
/// string `write_bridge_extension` persists.
fn bridge_extension() -> String {
    BRIDGE_EXTENSION_TEMPLATE.to_string()
}

pub(crate) fn write_bridge_extension(path: &std::path::Path) -> io::Result<()> {
    if !path.parent().is_some_and(std::path::Path::is_dir) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Pi bridge extension parent directory does not exist",
        ));
    }
    crate::mcp_broker::write_protected_str(path, &bridge_extension())
}

fn bridge_extension_path(runtime_dir: &Path) -> PathBuf {
    let serial = BRIDGE_EXTENSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    runtime_dir.join(format!("devboule-pi-bridge-{serial}.ts"))
}

fn remove_bridge_extension(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != io::ErrorKind::NotFound {
            eprintln!(
                "could not remove Pi bridge extension {}: {error}",
                path.display()
            );
        }
    }
}

/// The bridge announce (S5/S8 seam): the `session_start` notify the bridge sends,
/// the only out-of-band readiness signal (no rpc tool enumeration exists —
/// spike-measured). S8 uses it plus the in-child `tools/list` round-trip;
/// S5 logs its absence when tools were expected and proceeds (never blocks).
fn is_bridge_notify(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("extension_ui_request")
        && value.get("method").and_then(Value::as_str) == Some("notify")
        && value.get("message").and_then(Value::as_str) == Some("devboule-mcp-bridge")
}

/// The pi carrier seam (S4 shape, S5 body): from the launch config the broker
/// minted, the env the child needs and the owned bridge file. `arg_additions`
/// stays empty on purpose — the `-e` injection understands `--` and `--mode`
/// placement (`spawn_args`), so a verbatim argv splice from shared code would
/// break it; the bridge path travels as `owned_paths[0]` instead.
///
/// Token travels as child **env**, never argv. Until the provider trait lands
/// this is a free function with the trait's exact signature, so adoption is a move.
pub(crate) fn mcp_launch(
    config: &crate::mcp_broker::McpLaunchConfig,
    runtime_dir: &Path,
) -> Result<crate::mcp_broker::McpProviderConfig, WireError> {
    let bridge_path = bridge_extension_path(runtime_dir);
    write_bridge_extension(&bridge_path).map_err(|error| {
        remove_bridge_extension(&bridge_path);
        WireError::new(
            ErrorCode::Io,
            format!("Could not write the Pi bridge extension: {error}"),
        )
    })?;
    Ok(crate::mcp_broker::McpProviderConfig {
        env_additions: vec![
            (
                crate::mcp_broker::MCP_URL_ENV.to_string(),
                config.url.clone(),
            ),
            (
                crate::mcp_broker::MCP_TOKEN_ENV.to_string(),
                config.bearer().to_string(),
            ),
        ],
        arg_additions: Vec::new(),
        owned_paths: vec![bridge_path],
        owned_dirs: Vec::new(),
    })
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

/// The pi argv (S5): `--mode rpc` injected when the caller did not name it,
/// then our `-e` extensions — permission first, bridge second (the spike's
/// measured order; no ordering effects observed). Caller `-e`/`--extension`
/// forms and `--` handling are preserved: everything splices before `--`.
/// `bridge_path` is `None` until the broker mints a launch config (S9); with
/// `None` the argv is exactly the S3 shape (permission only) — zero behaviour
/// change while the gate is closed.
fn spawn_args(
    command: &PtyCommand,
    permission_path: &Path,
    bridge_path: Option<&Path>,
) -> Result<Vec<String>, WireError> {
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
    let mut extensions = vec![
        "-e".to_string(),
        permission_path.to_string_lossy().into_owned(),
    ];
    if let Some(bridge) = bridge_path {
        extensions.push("-e".to_string());
        extensions.push(bridge.to_string_lossy().into_owned());
    }
    let index = option_end(&args);
    args.splice(index..index, extensions);
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

/// The mode the daemon delivers when a create names none — the same default
/// `validate_delivery` and the spawn seed read, so the marker and the child
/// cannot disagree about what an absent mode means.
pub(crate) const DEFAULT_MODE: &str = "ask";

/// One entry of the daemon's own Pi mode vocabulary, and the answer the
/// `unattended` marker derives from it.
///
/// The vocabulary and the marker's dictionary are **one table**: the mode
/// list the manifest presents, the ids `validate_delivery` and `set_mode`
/// admit, and the marker's answers all come from here, so a new Pi mode
/// cannot be added without an answer — the `unattended` field is required by
/// the type, and there is no fall-through to be silent in. This is route-B
/// knowledge and it lives here, in the family that writes the permission
/// extension, never in a central table of mode names.
struct PiMode {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    /// `bypass` is route A and route B in one mode: it is one of the
    /// provider-agnostic ids the daemon's own broker answers, and the
    /// mechanism is the permission extension **this daemon writes** ceasing
    /// to gate. `ask` stops at the human for every tool.
    unattended: UnattendedState,
}

const PI_MODES: &[PiMode] = &[
    PiMode {
        id: "bypass",
        name: "Bypass",
        description: "Tools run without asking (Pi's native behaviour)",
        unattended: UnattendedState::Yes,
    },
    PiMode {
        id: "ask",
        name: "Always ask",
        description: "Ask before every tool call",
        unattended: UnattendedState::No,
    },
];

pub(super) fn mode_is_known(mode_id: &str) -> bool {
    PI_MODES.iter().any(|mode| mode.id == mode_id)
}

fn available_mode_views() -> Vec<SessionModeView> {
    PI_MODES
        .iter()
        .map(|mode| SessionModeView {
            id: mode.id.to_string(),
            name: mode.name.to_string(),
            description: Some(mode.description.to_string()),
        })
        .collect()
}

/// The marker's answer for one delivered Pi mode: the table walk above, with
/// the daemon's own default for a create that named none.
///
/// A mode id the table does not carry is a mode the daemon never authored —
/// it cannot be judged, and the answer is `unknown`, never `no`. The
/// delivery validation refuses such a mode before a child exists, so a
/// surviving child should never hit the miss; the miss arm exists so the
/// derivation itself stays honest if it ever is reached.
pub(crate) fn unattended_answer(delivered_mode: Option<&str>) -> UnattendedState {
    let mode_id = delivered_mode.unwrap_or(DEFAULT_MODE);
    PI_MODES
        .iter()
        .find(|mode| mode.id == mode_id)
        .map(|mode| mode.unattended)
        .unwrap_or(UnattendedState::Unknown)
}

/// The creation-time refusals Pi can make before a process exists: the mode
/// must be one of Pi's own, and an `autoAccept` tick demands a mode that
/// will not ask the human. `bypass` is the one Pi mode the daemon's own
/// broker answers (the injected extension stops gating), so a profile that
/// ticks the toggle and names `ask` asks the child to ask and not to ask at
/// once — the refusal is the answer; a substitution is not.
/// The tick half of [`validate_delivery`] as one predicate, shared with the
/// tests that cross it against the pre-card gate (the re-audit's P1): the
/// gate's `Contradicts` for pi must name exactly the pairs this refuses.
pub(crate) fn tick_contradicts(delivery: &ProfileDelivery) -> bool {
    let mode_id = delivery.mode_id.as_deref().unwrap_or(DEFAULT_MODE);
    delivery.auto_accept && !crate::provider_catalog::mode_is_auto_answered(mode_id)
}

pub(super) fn validate_delivery(delivery: &ProfileDelivery) -> Result<(), WireError> {
    // Pi has no permission gate of its own: the gate is the TypeScript
    // extension spawn writes and passes to the child. Inheriting Pi's native
    // behaviour as the default meant the surface that authorises "Create or
    // overwrite a file" never asked, so a session nobody asked a mode for
    // starts in "ask". The chip is the only way back to "bypass", and it is
    // a deliberate click.
    let mode_id = delivery
        .mode_id
        .as_deref()
        .unwrap_or(DEFAULT_MODE)
        .to_string();
    if !mode_is_known(&mode_id) {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("Pi session mode '{mode_id}' is not available."),
        ));
    }
    if tick_contradicts(delivery) {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!(
                "the profile asks Pi to approve its own permission prompts and also to start in mode '{mode_id}', which asks the human; the two contradict, so the creation is refused"
            ),
        ));
    }
    Ok(())
}

/// Spawn pi (S5 wiring, S9 live): `mcp` is the broker's launch config when the
/// session was registered for MCP tools, `None` otherwise. `None` is exactly
/// the old behaviour — permission extension only, no bridge, no broker env —
/// and stays the road for unregistered sessions (Terminal has no road at all).
/// `Some` writes the bridge beside the permission file, appends the second `-e`, and
/// joins `DEVBOULE_MCP_URL` + `DEVBOULE_MCP_TOKEN` onto the child env (never
/// argv). The bridge announce is required for nothing at spawn: absence when
/// tools were expected is logged (detected, not discovered) and proceeds —
/// S8 marks it `Unverified`. First-class tools, never a proxy; the adapter's
/// generic proxy + second permission system stays the documented fallback only.
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
        .unwrap_or(DEFAULT_MODE)
        .to_string();
    let extension_path = permission_extension_path(state.sessions.runtime_dir());
    // The carrier, only when the broker minted one (S9: registered sessions;
    // unregistered spawns keep the `None` road below, byte-identical).
    let bridge = match mcp.as_ref() {
        Some(config) => Some(mcp_launch(config, state.sessions.runtime_dir())?),
        None => None,
    };
    let bridge_path: Option<PathBuf> = bridge
        .as_ref()
        .and_then(|carrier| carrier.owned_paths.first().cloned());
    // S4 seam invariants, pinned loudly: pi carries no verbatim argv additions
    // (the `-e` splice understands `--`/`--mode` placement, so shared code must
    // never splice argv for it) and no owned dirs (one bridge file; S6 fills the
    // dir half for Codex). A carrier violating either fails here, not in the child.
    if let Some(carrier) = bridge.as_ref() {
        assert!(
            carrier.arg_additions.is_empty() && carrier.owned_dirs.is_empty(),
            "pi carrier is env + one bridge file, nothing else"
        );
    }
    let remove_bridge = |bridge_path: &Option<PathBuf>| {
        if let Some(path) = bridge_path {
            remove_bridge_extension(path);
        }
    };
    let args = spawn_args(&command, &extension_path, bridge_path.as_deref())?;
    if let Err(error) = write_permission_extension(&extension_path) {
        remove_permission_extension(&extension_path);
        remove_bridge(&bridge_path);
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
    // The carrier env, only when the broker minted one: URL + token as child
    // env (never argv — the argv token-free assertion in S5 tests pins this).
    if let Some(carrier) = bridge.as_ref() {
        for (key, value) in &carrier.env_additions {
            process.env(key, value);
        }
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
            remove_bridge(&bridge_path);
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
            remove_bridge(&bridge_path);
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
            remove_bridge(&bridge_path);
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
        remove_bridge(&bridge_path);
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
        remove_bridge(&bridge_path);
        WireError::new(ErrorCode::Io, "Pi did not provide stdin.")
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        terminate_process(&mut child);
        remove_permission_extension(&extension_path);
        remove_bridge(&bridge_path);
        WireError::new(ErrorCode::Io, "Pi did not provide stdout.")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        terminate_process(&mut child);
        remove_permission_extension(&extension_path);
        remove_bridge(&bridge_path);
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
            remove_bridge(&bridge_path);
            drop(process_job);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not read Pi stdout: {error}"),
            ));
        }
    };
    let handshake = match perform_handshake(&mut stdout, &stdin, &next_id, &mode_id) {
        Ok(handshake) => handshake,
        Err(error) => {
            terminate_shared_process(&process);
            remove_permission_extension(&extension_path);
            remove_bridge(&bridge_path);
            drop(process_job);
            return Err(error);
        }
    };

    let controls = Arc::new(Mutex::new(HashMap::new()));
    let permission_extension_active = Arc::new(AtomicBool::new(
        handshake.deferred.iter().any(is_ready_notify),
    ));
    // S5/S8 seam: the bridge announce is the only out-of-band readiness signal.
    // Absence when tools were expected is detected, not discovered — one honest
    // line — and the spawn proceeds: S8 marks it `Unverified`, and prompts never
    // wait. (Contrast the permission gate below, which refuses: a missing
    // permission gate is unsafe, a missing bridge is merely tool-less.)
    let bridge_active = handshake.deferred.iter().any(is_bridge_notify);
    if bridge.is_some() && !bridge_active {
        eprintln!(
            "pi bridge expected but its announce is absent: the child starts without Devboule tools (unverified)"
        );
    }
    if mode_id == "ask" && !permission_extension_active.load(Ordering::Acquire) {
        terminate_shared_process(&process);
        remove_permission_extension(&extension_path);
        remove_bridge(&bridge_path);
        drop(process_job);
        // A provider failure, not a profile refusal: the extension the
        // daemon injected never announced itself, which says something about
        // this Pi build and nothing about the profile. The code is what the
        // health recorder reads (`spawn_failure_is_provider_health`).
        return Err(WireError::new(
            ErrorCode::Io,
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
        bridge_path: bridge_path.clone(),
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
        remove_bridge(&bridge_path);
        WireError::new(ErrorCode::Io, format!("Could not drain Pi stderr: {error}"))
    })?;
    // The static prompt route reads the live model from the same catalog the
    // switcher keeps, so the two share one `Arc`. The switcher is built before
    // the session is assembled because the delivery runs through it: the same
    // validated rpc the runtime switch uses is what puts the profile's model
    // and thinking level in force, and a refusal there tears the child down
    // before it was ever a session.
    let catalog = Arc::new(Mutex::new(handshake.catalog));
    let mode_id_state = Arc::new(Mutex::new(mode_id.clone()));
    let switcher = PiSwitcher {
        control: Arc::clone(&control),
        catalog: Arc::clone(&catalog),
        mode_id: Arc::clone(&mode_id_state),
        permission_extension_active: Arc::clone(&permission_extension_active),
    };
    // The delivery is NOT awaited here. `set_model` is an awaited control
    // rpc, and the only code that can deliver its answer is the session
    // reader thread — which `start_spawned_session` starts after this
    // function returns. An awaited call at this point stalls fifteen seconds
    // against a deliverer that does not exist and then kills every child a
    // profile creates (the R2a audit's F1). The rpc travels as a hook on the
    // `SpawnedSession` instead; the hook runs once that reader is live, and
    // a refusal there still tears the child down before it can answer
    // anything.
    let pending_delivery = pending_pi_delivery(&switcher, &delivery);
    let static_prompt = Arc::new(PiStaticPrompt::new(
        Arc::clone(&stdin),
        Arc::clone(&next_id),
        Arc::clone(&catalog),
    ));
    Ok(SpawnedSession {
        process_job,
        master: None,
        killer: Box::new(killer),
        switcher: Some(Box::new(switcher)),
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
        pending_delivery,
        pending_codex_verify: None,
    })
}

/// The profile's delivery as the rpc it is: the switcher's awaited
/// `set_model`, packaged for [`session::start_spawned_session`] to run once
/// the session reader thread can deliver the answer. `None` when the profile
/// names neither a model nor a thinking level. The switcher this clones
/// shares the catalog and mode state with the one on the session, so the
/// delivery and the runtime switch move the same state.
fn pending_pi_delivery(
    switcher: &PiSwitcher,
    delivery: &ProfileDelivery,
) -> Option<Box<dyn FnOnce() -> Result<(), WireError> + Send>> {
    if delivery.model_id.is_none() && delivery.thinking_option_id.is_none() {
        return None;
    }
    let switcher = switcher.clone_switcher();
    let model_id = delivery.model_id.clone();
    let effort = delivery.thinking_option_id.clone();
    Some(Box::new(move || {
        switcher.set_model(model_id.as_deref(), effort.as_deref())
    }))
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
            // One vocabulary, one table: the manifest presents exactly the
            // modes [`validate_delivery`], `set_mode` and the `unattended`
            // marker judge.
            available_modes: available_mode_views(),
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

/// The fields of one Pi steer frame: the text, and the literal empty `images`
/// array Pi's own `steer(text, images)` sends. Deliberately unlike
/// `pi_prompt_frame`, which omits `images` when it carries none. The `id` and
/// the `type` come from [`pi_control_frame`], built with the id the round-trip
/// registered.
fn pi_steer_fields(text: &str) -> serde_json::Value {
    serde_json::json!({"text": text, "images": []})
}

/// One Pi control frame: the id the response will name, the command, then the
/// fields that command carries, in that order.
fn pi_control_frame(id: &str, command: &str, fields: serde_json::Value) -> serde_json::Value {
    let mut frame = serde_json::json!({"id": id, "type": command});
    if let Some(object) = fields.as_object() {
        frame
            .as_object_mut()
            .expect("control frame is an object")
            .extend(object.clone());
    }
    frame
}

/// A steer Pi will not take is `Ok(false)`, not a failure: a Pi whose build has
/// no such command answers `success: false` with `Unknown command: steer`, and
/// the caller's job then is the pre-existing fallback, not an error. Everything
/// else — a timeout, a broken pipe, a rejection this code does not recognise —
/// stays an `Err`: the honest answer is that the steer's fate is unknown.
fn map_pi_steer_error(error: WireError) -> Result<bool, WireError> {
    // Case-insensitive on purpose (A2-11): a provider build may spell the
    // refusal `Unknown command: steer` or `unknown command: steer`, and the two
    // mean the same thing — this build has no such command, so the caller's
    // pre-existing fallback is the answer, not an error. The comparison
    // lowercases the message into an owned `String`; the `WireError` is moved
    // into the `Err` arm unchanged, so the message a caller sees is the
    // provider's own spelling.
    if error
        .message
        .to_ascii_lowercase()
        .contains("unknown command")
    {
        Ok(false)
    } else {
        Err(error)
    }
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

    /// The references join this plan's text as path lines, never as `images[]`
    /// entries: see `session::push_reference_path_lines`.
    fn append_reference_path_lines(&mut self, reference_paths: &[PathBuf]) {
        super::push_reference_path_lines(&mut self.plan.fallback_text, reference_paths);
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
    bridge_path: Option<PathBuf>,
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
        if let Some(bridge) = self.bridge_path.as_deref() {
            remove_bridge_extension(bridge);
        }
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self {
            process: Arc::clone(&self.process),
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            permission_broker: Arc::clone(&self.permission_broker),
            cancelled: Arc::clone(&self.cancelled),
            extension_path: self.extension_path.clone(),
            bridge_path: self.bridge_path.clone(),
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

    /// Register the pending sender and write one command, answering the id the
    /// response will name and the channel that response arrives on.
    ///
    /// Split from the wait so a caller that must hold a lock across the *write*
    /// — a steer, admitted under the runtime's turn-hold — does not hold it
    /// across the answer: the answer is delivered by the reader thread, which a
    /// lock held across the wait would block on the very response it has to
    /// deliver.
    fn begin(
        &self,
        command: &str,
        fields: Value,
    ) -> Result<(String, mpsc::Receiver<Result<Value, String>>), WireError> {
        let id = format!("c-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| WireError::new(ErrorCode::Io, "Pi control map is unavailable."))?
            .insert(id.clone(), tx);
        let frame = pi_control_frame(&id, command, fields);
        if let Err(error) = send_json(&self.stdin, &frame, "Pi") {
            let _ = self.pending.lock().map(|mut pending| pending.remove(&id));
            return Err(error);
        }
        Ok((id, rx))
    }

    /// Wait for the response one command is answered with, refusing a
    /// `success: false` the way the control protocol spells a rejection.
    fn await_response(
        &self,
        command: &str,
        id: &str,
        response: mpsc::Receiver<Result<Value, String>>,
    ) -> Result<Value, WireError> {
        let response = response.recv_timeout(RESPONSE_TIMEOUT).map_err(|error| {
            let _ = self.pending.lock().map(|mut pending| pending.remove(id));
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

    fn request(&self, command: &str, fields: Value) -> Result<Value, WireError> {
        let (id, response) = self.begin(command, fields)?;
        self.await_response(command, &id, response)
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

    /// Wake every waiter still registered with the reason the control channel
    /// ended (A2-02).
    ///
    /// A response can no longer arrive for any of them — the child's output is
    /// what delivers responses, and it is over — so a waiter left registered
    /// would sit out its whole timeout for an answer that cannot come. The map
    /// is drained under its lock and each sender is answered outside it.
    fn fail_pending(&self, message: &str) {
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        let waiters: Vec<Sender<Result<Value, String>>> =
            pending.drain().map(|(_, sender)| sender).collect();
        drop(pending);
        for sender in waiters {
            let _ = sender.send(Err(message.to_string()));
        }
    }
}

struct PiSwitcher {
    control: Arc<PiControl>,
    catalog: Arc<Mutex<PiCatalog>>,
    mode_id: Arc<Mutex<String>>,
    permission_extension_active: Arc<AtomicBool>,
}

struct PiSteerer {
    control: Arc<PiControl>,
}

impl SessionSteerer for PiSteerer {
    fn steer_active_turn(
        &mut self,
        text: &str,
        turn: &mut TurnToken<'_>,
    ) -> Result<bool, WireError> {
        // The steer goes through the id-correlated round-trip rather than a
        // bare write (S4-01 of the provider fix): a write only says the bytes
        // reached the pipe, and Pi's `success: false` response would then be
        // dropped as an unclaimed frame while the daemon recorded a steer Pi
        // never took. The write itself stays under the caller's turn-hold; the
        // wait does not, because the reader thread delivers the response.
        let (id, response) =
            turn.write_then_release(|| self.control.begin("steer", pi_steer_fields(text)))?;
        self.control
            .await_response("steer", &id, response)
            .map(|_response| true)
            .or_else(map_pi_steer_error)
    }

    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(Self {
            control: Arc::clone(&self.control),
        })
    }
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
        // Absent vocabulary and unknown id are two different refusals: a
        // provider that publishes no models cannot deliver any choice, while
        // a published list that lacks the named id is a typo the human can
        // fix. Collapsing them sends someone hunting a typo when the provider
        // simply has no dial.
        if current.models.is_empty() {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Pi publishes no models; the profile names one, so the creation is refused",
            ));
        }
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
            if levels.is_empty() {
                // Absence, not mismatch: the model publishes no thinking
                // levels at all, so there is no list the name could have
                // been a typo from.
                let error = WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("Pi model '{model_id}' has no thinking options; the profile names one, so the creation is refused"),
                );
                return Err(self.rollback_error(&current, error));
            }
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
        if !mode_is_known(mode_id) {
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

    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(PiSteerer {
            control: Arc::clone(&self.control),
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
        // A placeholder the daemon overwrites with the session's stored origin
        // before the request leaves for a subscriber.
        origin: devboule_protocol::SessionOrigin::unknown(),
        create_agent: None,
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
        // The child's output is over, so the control channel is: every waiter
        // still holding a response channel is answered here with what that end
        // means (A2-02), rather than being left to time out on a reply the
        // reader can no longer deliver.
        self.control
            .fail_pending("Pi control channel closed before the response arrived.");
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

/// Whether pi calls `name` without raising a human confirm (S3).
///
/// Not "read only": the set holds reads (`read`, the roster) and writes
/// (`devboule_send_message`, `devboule_create_agent`) alike. What unites them
/// is that the daemon decides — the broker's own card, roster or door — so the
/// generic human gate must not fire. `write`/`bash` stay confirm-requiring.
fn is_unmediated_tool(name: &str) -> bool {
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
                            publish_stderr_line(&runtime, data);
                        }
                        Err(_) => return,
                    }
                }
            })
    }
}

/// One stderr chunk to the transcript (broker-4): published through the one
/// redactor ACP, Claude and Codex already use — a pi child that echoes its
/// environment must not land the broker token in any observer's transcript.
/// (Found beside the Codex gap: same defect, same fix. No belt here — the
/// invalid-configuration marker is Codex's sentence, not pi's.)
fn publish_stderr_line(runtime: &SessionRuntime, data: String) {
    let _ = runtime.publish_agent_event(
        SessionEvent::AgentStderr {
            data: runtime.redact_mcp_text(&data),
        },
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::{
        bridge_extension, carried_pi_mime_types, is_bridge_notify, is_ready_notify, mcp_launch,
        perform_handshake, permission_extension_path, permission_request_from_ui, pi_control_frame,
        pi_delivery, pi_image_entry, pi_permission_sender, pi_prompt_frame, pi_steer_fields,
        plan_pi_prompt, spawn_args, thinking_level_allowed, write_permission_extension, PiCatalog,
        PiControl, PiReader, PiStaticPrompt, PiStdout, PiSteerer, PiSwitcher,
    };
    use crate::acp_view::PromptCapabilityState;
    use crate::attachment_store::AttachmentStore;
    use crate::pi_view::events_from_line;
    use crate::raster_metadata::{clean_png, png_with_text_chunk, vector_input, vector_output};
    use crate::session::{
        ModelSwitcher, PtyCommand, ReaderDispatch, SessionRuntime, StaticImageSink,
    };
    // The shared admission helper (A2-03): one place, so the Pi and the Codex
    // steer tests exercise the same token `with_active_turn` hands out.
    use crate::test_support::steer_through_the_turn;
    use devboule_protocol::{PromptAttachment, SessionEvent};
    use std::collections::HashMap;
    use std::io::BufRead;
    use std::path::Path;
    use std::process::ChildStdin;
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
        // A2-13: the suite skips without `node` instead of turning a machine
        // that lacks it into a red build.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
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
        let args = spawn_args(&command, path, None).expect("Pi args");
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
        let args = spawn_args(&command, path, None).expect("caller extensions are valid");
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
            spawn_args(&command, Path::new("permission.ts"), None)
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
            spawn_args(&command, Path::new("permission.ts"), None)
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
        // A2-13: a machine without `node` skips this rather than failing it.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
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
        // A2-13: a machine without `node` skips this rather than failing it.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
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
    fn pi_broker_tools_are_unmediated_and_walked() {
        // S3 walking test: every tool the broker serves is classified exactly
        // once by the same constants the extension renders — unmediated with a
        // reason (all six today), never silently inheriting either answer. A
        // seventh broker tool with no row here fails the first assertion; a
        // `devboule_*` name missing from the unmediated set fails the second.
        for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
            assert!(
                super::is_unmediated_tool(name),
                "broker tool {name} must be unmediated (daemon-decided)"
            );
        }
        // The rendered set states the fact: the new identifier is present and
        // the old read-only name survives nowhere.
        let rendered = super::permission_extension();
        assert!(
            rendered.contains("unmediated"),
            "the extension renders the unmediated set"
        );
        assert!(
            !rendered.contains("__UNMEDIATED_TOOLS__"),
            "the placeholder is substituted, not left verbatim"
        );
        assert!(
            !rendered.contains("readOnly") && !rendered.contains("__READ_ONLY_TOOLS__"),
            "the false read-only name survives nowhere"
        );
        // The gate itself is unchanged: writes still ask, reads still pass.
        assert!(!super::is_unmediated_tool("write"));
        assert!(!super::is_unmediated_tool("bash"));
        assert!(!super::is_unmediated_tool("tool-futuro"));
        for name in ["read", "grep", "find", "ls"] {
            assert!(super::is_unmediated_tool(name), "{name} stays unmediated");
        }
        // Every rendered name is a known one: nothing unmediated by accident.
        let start = rendered.find('[').expect("rendered tool list");
        let end = rendered[start..].find(']').expect("rendered tool list end") + start;
        let list: Vec<String> =
            serde_json::from_str(&rendered[start..=end]).expect("rendered list is JSON");
        for name in &list {
            let known_pi = super::PI_TOOL_POLICIES.iter().any(|tool| tool.name == name);
            assert!(known_pi, "rendered {name} comes from the policy table");
        }
        for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
            assert!(
                list.iter().any(|rendered| rendered == name),
                "broker tool {name} reaches the rendered extension"
            );
        }
    }

    #[test]
    fn pi_bridge_template_serves_the_broker_tools() {
        // S5 walking test for the bridge: every served tool's name and verbatim
        // description reaches the exact string `write_bridge_extension` persists —
        // a seventh tool, or a catalog rewording without a bridge edit, fails.
        // Hygiene markers ride the same test: dual Accept, named timeout, bridge
        // announce, env-identity Bearer, result/error + 202 handling.
        let template = bridge_extension();
        assert!(template.contains("import { Type }"), "typebox builders");
        for (name, description) in crate::provider_catalog::MCP_BROKER_TOOLS {
            assert!(
                template.contains(name),
                "bridge registers broker tool {name}"
            );
            assert!(
                template.contains(description),
                "bridge carries the catalog description for {name}"
            );
        }
        assert!(
            template.contains("Accept\": \"application/json, text/event-stream\"")
                || template.contains("Accept: \"application/json, text/event-stream\""),
            "dual Accept takes the broker JSON branch"
        );
        assert!(
            template.contains("MCP_TIMEOUT_MS") && template.contains("AbortSignal.timeout"),
            "every MCP fetch races the named timeout (spike S3b hung 80 s without one)"
        );
        assert!(
            template.contains("devboule-mcp-bridge"),
            "bridge announce rides session_start like the permission channel"
        );
        assert!(
            template.contains("mcpRequest(\"tools/list\", {}, undefined)"),
            "session_start proves in-band with an authenticated tools/list (S8 producer)"
        );
        assert!(
            template.contains("process.env.DEVBOULE_MCP_URL")
                && template.contains("process.env.DEVBOULE_MCP_TOKEN"),
            "identity by environment"
        );
        assert!(
            template.contains("Authorization: `Bearer ${MCP_TOKEN}`")
                || template.contains("Authorization\": `Bearer ${MCP_TOKEN}`"),
            "Bearer flies on every request (spike S5)"
        );
        assert!(
            template.contains("payload.error") || template.contains("payload && payload.error"),
            "RPC errors ride HTTP 200: parse result/error, never the status"
        );
        assert!(
            template.contains("202"),
            "202-empty is success without a result (never parsed, never failed)"
        );
        assert!(
            template.contains("devboule broker unreachable at"),
            "failures name the broker URL + cause, never bare fetch failed"
        );
        assert!(
            !template.contains("SPIKE_DUMP") && !template.contains("getSystemPrompt"),
            "no spike instrumentation ships"
        );
        assert!(
            !template.contains("process.argv"),
            "argv never carries identity"
        );
    }

    #[test]
    fn pi_bridge_announce_is_detected_not_discovered() {
        // No rpc tool enumeration exists (spike-measured): the notify is the only
        // out-of-band readiness signal. S8 consumes this plus the in-child round-trip.
        let bridge = serde_json::json!({
            "type": "extension_ui_request",
            "method": "notify",
            "message": "devboule-mcp-bridge",
        });
        assert!(is_bridge_notify(&bridge));
        let permission = serde_json::json!({
            "type": "extension_ui_request",
            "method": "notify",
            "message": "devboule-permission-channel",
        });
        assert!(!is_bridge_notify(&permission));
        assert!(is_ready_notify(&permission));
        assert!(!is_ready_notify(&bridge));
        assert!(!is_bridge_notify(&serde_json::json!({"type": "session"})));
    }

    #[test]
    fn pi_spawn_args_put_the_bridge_second_and_keep_callers() {
        // S5 argv shape: permission first, bridge second (the spike's measured
        // order), everything before `--`, caller forms preserved, token-free.
        let command = PtyCommand::new(
            "pi",
            vec!["--extension".to_string(), "first.ts".to_string()],
            std::env::current_dir().expect("cwd"),
            Vec::new(),
        );
        let permission = Path::new(r"C:\runtime\devboule-pi-permissions-1.ts");
        let bridge = Path::new(r"C:\runtime\devboule-pi-bridge-2.ts");
        let args = spawn_args(&command, permission, Some(bridge)).expect("bridge args");
        let dashes: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, arg)| *arg == "-e")
            .map(|(index, _)| index)
            .collect();
        assert_eq!(dashes.len(), 2, "permission -e plus bridge -e: {args:?}");
        assert_eq!(args[dashes[0] + 1], permission.to_string_lossy());
        assert_eq!(args[dashes[1] + 1], bridge.to_string_lossy());
        assert!(dashes[0] < dashes[1], "permission first, bridge second");
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--extension".to_string(), "first.ts".to_string()]));
        // Token-free argv: the bearer travels as env, never here.
        for arg in &args {
            assert!(!arg.contains("secret-bearer"), "no secret in argv: {arg}");
        }
        // And `None` stays the S3 shape: exactly one -e.
        let plain = spawn_args(&command, permission, None).expect("plain args");
        assert_eq!(plain.iter().filter(|arg| *arg == "-e").count(), 1);
    }

    #[test]
    fn pi_mcp_launch_separates_env_from_argv() {
        // S4 seam body: env carries URL + token values, argv carries nothing,
        // the bridge file exists with the served names, owned_paths names it.
        let dir =
            std::env::temp_dir().join(format!("devboule-pi-mcp-launch-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let config = crate::mcp_broker::McpLaunchConfig::for_test(
            "http://127.0.0.1:4321/mcp",
            "secret-bearer-launch",
        );
        let carrier = mcp_launch(&config, &dir).expect("carrier");
        assert!(carrier.arg_additions.is_empty(), "no verbatim argv splice");
        assert!(carrier.owned_dirs.is_empty(), "pi owns no dirs");
        let env: std::collections::HashMap<_, _> = carrier.env_additions.iter().cloned().collect();
        assert_eq!(
            env.get(crate::mcp_broker::MCP_URL_ENV).map(String::as_str),
            Some("http://127.0.0.1:4321/mcp")
        );
        assert_eq!(
            env.get(crate::mcp_broker::MCP_TOKEN_ENV)
                .map(String::as_str),
            Some("secret-bearer-launch")
        );
        assert_eq!(carrier.owned_paths.len(), 1);
        let bridge = std::fs::read_to_string(&carrier.owned_paths[0]).expect("bridge file");
        for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
            assert!(bridge.contains(name), "persisted bridge serves {name}");
        }
        assert!(
            !bridge.contains("secret-bearer-launch"),
            "no secret on disk"
        );
        let name = carrier.owned_paths[0]
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        assert!(
            name.starts_with("devboule-pi-bridge-") && name.ends_with(".ts"),
            "owned bridge name the S4 sweep covers: {name}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pi_bridge_drives_the_broker_shape_through_a_stub_pi() {
        // S5 executable analogue of spike S2/S3/S5 (which measured against the
        // real pi binary + stub broker/model): the persisted bridge file loads in
        // node against a stubbed `pi` object and a stubbed `fetch`, proving the
        // registerTool wiring, the Bearer + dual-Accept + string-body shape, the
        // URL-naming failure text, the RPC-error-on-200 parse, and the closed
        // schemas — without a pi binary or a socket.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        let dir =
            std::env::temp_dir().join(format!("devboule-pi-bridge-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("bridge.mjs"), bridge_extension()).expect("bridge file");
        // A minimal `typebox` stub: the bridge only needs the builders to record
        // their arguments; the assertions below read the recorded schemas back.
        // (The real pi resolves its bundled typebox; spelling is identical.)
        let stub_dir = dir.join("node_modules").join("typebox");
        let _ = std::fs::create_dir_all(&stub_dir);
        std::fs::write(
            stub_dir.join("package.json"),
            r#"{"name":"typebox","version":"0.0.0-test","type":"module","main":"index.js"}"#,
        )
        .expect("stub package");
        std::fs::write(
            stub_dir.join("index.js"),
            r#"
export const Type = {
  Object: (properties, opts = {}) => ({ type: "object", properties, ...opts }),
  String: (opts = {}) => ({ type: "string", ...opts }),
  Boolean: (opts = {}) => ({ type: "boolean", ...opts }),
  Optional: (inner) => ({ ...inner, optional: true }),
  Record: (k, v, opts = {}) => ({ type: "object", record: true, ...opts }),
  Union: (anyOf) => ({ anyOf }),
  Literal: (value) => ({ const: value }),
};
"#,
        )
        .expect("stub index");
        let script = r#"
(async () => {
  const path = require("path");
  const { pathToFileURL } = require("url");
  const tmp = process.argv[1];
  const calls = [];
  let behavior = "ok";
  global.fetch = async (url, opts) => {
    calls.push({ url, headers: opts.headers, body: opts.body });
    if (behavior === "refused") {
      const error = new Error("fetch failed");
      error.cause = { code: "ECONNREFUSED" };
      throw error;
    }
    if (behavior === "rpc-error") {
      return { ok: true, status: 200, text: async () => JSON.stringify({ jsonrpc: "2.0", id: 1, error: { code: -32601, message: "Unknown tool" } }) };
    }
    const req = JSON.parse(opts.body);
    if (req.method === "notifications/initialized") {
      return { ok: true, status: 202, text: async () => "" };
    }
    if (req.method === "initialize") {
      return { ok: true, status: 200, text: async () => JSON.stringify({ jsonrpc: "2.0", id: req.id, result: { protocolVersion: "2025-06-18", capabilities: {}, serverInfo: { name: "devboule", version: "1" } } }) };
    }
    return { ok: true, status: 200, text: async () => JSON.stringify({ jsonrpc: "2.0", id: req.id, result: { content: [{ type: "text", text: "STUB-OK" }], structuredContent: {} } }) };
  };
  const tools = {};
  const handlers = {};
  const notifies = [];
  const pi = {
    on(name, handler) { handlers[name] = handler; },
    registerTool(definition) { tools[definition.name] = definition; },
  };
  const extension = await import(pathToFileURL(path.join(tmp, "bridge.mjs")).href);
  extension.default(pi);
  await handlers.session_start({}, { ui: { notify: (message, kind) => notifies.push([message, kind]) } });
  if (!notifies.some(([message]) => message === "devboule-mcp-bridge")) process.exit(11);
  for (const name of ["devboule_list_agents", "devboule_list_profiles", "devboule_send_message", "devboule_create_agent", "devboule_set_agent_profile", "devboule_answer_permission"]) {
    if (!tools[name]) { console.error("missing tool " + name); process.exit(12); }
  }
  // S8 producer: session_start proves in-band with an authenticated tools/list.
  const listCalls = calls.filter((call) => {
    try { return JSON.parse(call.body).method === "tools/list"; } catch { return false; }
  });
  if (listCalls.length < 1) process.exit(25);
  if (listCalls[0].headers.Authorization !== `Bearer ${process.env.DEVBOULE_MCP_TOKEN}`) process.exit(26);
  const result = await tools.devboule_list_agents.execute("t1", {}, undefined);
  const last = calls[calls.length - 1];
  if (last.headers.Authorization !== `Bearer ${process.env.DEVBOULE_MCP_TOKEN}`) process.exit(13);
  if (last.headers.Accept !== "application/json, text/event-stream") process.exit(14);
  if (typeof last.body !== "string") process.exit(15);
  if (last.url !== process.env.DEVBOULE_MCP_URL) process.exit(16);
  if (!JSON.stringify(result).includes("STUB-OK")) process.exit(17);
  behavior = "refused";
  try {
    await tools.devboule_list_agents.execute("t2", {}, undefined);
    process.exit(18);
  } catch (error) {
    const text = String(error && error.message ? error.message : error);
    if (!text.includes(process.env.DEVBOULE_MCP_URL) || text === "fetch failed") process.exit(19);
  }
  behavior = "rpc-error";
  try {
    await tools.devboule_list_agents.execute("t3", {}, undefined);
    process.exit(20);
  } catch (error) {
    if (!String(error && error.message ? error.message : error).includes("Unknown tool")) process.exit(21);
  }
  const create = tools.devboule_create_agent.parameters;
  if (create.additionalProperties !== false) process.exit(22);
  for (const key of ["profile", "title", "initialPrompt"]) {
    if (!(create.required || []).includes(key)) process.exit(23);
  }
  const outcome = (((tools.devboule_answer_permission.parameters || {}).properties || {}).outcome || {});
  const values = outcome.anyOf ? outcome.anyOf.map((entry) => entry.const) : outcome.enum;
  if (!values || !values.includes("allow_once") || !values.includes("deny")) process.exit(24);
  // S8 tolerance: a failed startup list breaks nothing — the announce fired and
  // later calls still work. Refused broker, second startup, must resolve.
  behavior = "refused";
  const notified = notifies.length;
  await handlers.session_start({}, { ui: { notify: (message, kind) => notifies.push([message, kind]) } });
  if (notifies.length !== notified + 1) process.exit(27);
})().catch((error) => { console.error(error); process.exit(3); });
"#;
        let output = std::process::Command::new("node")
            .args(["-e", script, &dir.to_string_lossy()])
            .env("DEVBOULE_MCP_URL", "http://127.0.0.1:4321/mcp")
            .env("DEVBOULE_MCP_TOKEN", "stub-token-bridge")
            .env("NO_COLOR", "1")
            .output()
            .unwrap_or_else(|error| {
                panic!("{}", node_unavailable("Pi bridge behavior test", &error))
            });
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            output.status.success(),
            "pi bridge behavior failed (exit={}): {}{}",
            output
                .status
                .code()
                .map_or_else(|| "no exit code".to_string(), |code| code.to_string()),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Attached-runtime helper mirroring ACP's (and Codex's): a runtime with a
    /// broker plus a subscription whose published events the test can pull.
    fn attached_runtime(
        session_id: &str,
        broker: Arc<super::super::permission_broker::PermissionBroker>,
    ) -> (
        Arc<SessionRuntime>,
        Arc<super::super::event_pull::ConnHandle>,
    ) {
        let runtime = SessionRuntime::for_acp(session_id.to_string(), None, Arc::clone(&broker));
        let conn = super::super::event_pull::ConnHandle::new(1);
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
    fn pi_bearer_is_redacted_from_stderr_before_delivery() {
        // Broker-4, pi half: same defect as Codex (a bearer in the child env
        // since S9), same fix, same proof. No belt here — the
        // invalid-configuration marker is Codex's sentence, not pi's.
        let broker =
            super::super::permission_broker::PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let (runtime, conn) = attached_runtime("stderr-redaction-pi", broker);
        runtime.set_mcp_bearer("opaque-bearer-pi".to_string());
        runtime.set_mcp_url("http://127.0.0.1:4567/mcp".to_string());
        super::publish_stderr_line(
            &runtime,
            "pi echoed Bearer opaque-bearer-pi at http://127.0.0.1:4567/mcp".to_string(),
        );
        let event = conn
            .pull_events()
            .into_iter()
            .find_map(|event| match event.envelope.event {
                SessionEvent::AgentStderr { data } => Some(data),
                _ => None,
            })
            .expect("stderr event");
        assert_eq!(event, "pi echoed Bearer [redacted] at [redacted]");
        // And a clean line passes through verbatim (redaction, not blanking).
        super::publish_stderr_line(&runtime, "pi did something ordinary".to_string());
        let clean = conn
            .pull_events()
            .into_iter()
            .find_map(|event| match event.envelope.event {
                SessionEvent::AgentStderr { data } => Some(data),
                _ => None,
            })
            .expect("second stderr event");
        assert_eq!(clean, "pi did something ordinary");
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
    fn pi_steer_frame_is_byte_exact_and_has_literal_empty_images() {
        // The frame `request` writes for a steer: the id it mints, the command,
        // then the fields — with the literal empty `images` array Pi's own
        // `steer(text, images)` sends, deliberately unlike `pi_prompt_frame`,
        // which omits `images` when it carries none.
        assert_eq!(
            serde_json::to_vec(&pi_control_frame("c-1", "steer", pi_steer_fields("hello")))
                .expect("frame"),
            br#"{"id":"c-1","type":"steer","text":"hello","images":[]}"#
        );
        assert!(pi_prompt_frame("p-test", "hello", &[])
            .get("images")
            .is_none());
    }

    /// A fake Pi that answers every control frame with a response naming the
    /// frame's own id, echoing the raw line it read. Pi answers commands this
    /// way; what differs between builds is `success` and the error it spells.
    fn fake_pi(
        script: &str,
    ) -> (
        std::process::Child,
        Arc<Mutex<Option<ChildStdin>>>,
        std::io::BufReader<std::process::ChildStdout>,
    ) {
        let mut child = node_command()
            .args(["-e", script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| {
                panic!("{}", node_unavailable("Pi steer round-trip test", &error))
            });
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
        (child, stdin, stdout)
    }

    /// A Pi whose build has no `steer` command, as its control protocol
    /// answers one.
    const FAKE_PI_UNKNOWN_STEER: &str = r#"
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    process.stdout.write(
      JSON.stringify({
        id: frame.id,
        type: "response",
        success: false,
        error: "Unknown command: steer",
        received: line,
      }) + "\n"
    );
  }
});
"#;

    /// A Pi that takes the steer, and echoes the frame it took.
    const FAKE_PI_STEERS: &str = r#"
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    process.stdout.write(
      JSON.stringify({
        id: frame.id,
        type: "response",
        success: true,
        received: line,
      }) + "\n"
    );
  }
});
"#;

    /// The pieces one round-trip test drives.
    struct AnsweringPi {
        child: std::process::Child,
        control: Arc<PiControl>,
        answers: Arc<Mutex<Vec<serde_json::Value>>>,
        reader: std::thread::JoinHandle<()>,
    }

    impl AnsweringPi {
        /// Stop the fake Pi, join its reader, and hand back what that reader
        /// read: each answer, in arrival order.
        fn answers(mut self) -> Vec<serde_json::Value> {
            let answers = self.answers.lock().expect("answers").clone();
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = self.reader.join();
            answers
        }
    }

    /// A fake Pi whose answers are delivered the way the real client delivers
    /// them: a reader thread turns each stdout line into a pending response, so
    /// the round-trip is correlated by id instead of timing out. Answers are
    /// recorded before delivery, so a test can assert on them the moment the
    /// steer returns.
    fn fake_pi_answering(script: &str) -> AnsweringPi {
        let (child, stdin, stdout) = fake_pi(script);
        let control = Arc::new(PiControl::new(stdin, Arc::new(AtomicU64::new(1))));
        let answers = Arc::new(Mutex::new(Vec::new()));
        let reader_control = Arc::clone(&control);
        let reader_answers = Arc::clone(&answers);
        let reader = std::thread::spawn(move || {
            let mut stdout = stdout;
            let mut line = String::new();
            loop {
                line.clear();
                match std::io::BufRead::read_line(&mut stdout, &mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
                let Ok(answer) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if let Ok(mut answers) = reader_answers.lock() {
                    answers.push(answer.clone());
                }
                let _ = reader_control.deliver(&answer);
            }
        });
        AnsweringPi {
            child,
            control,
            answers,
            reader,
        }
    }

    /// A reader with no child behind it, for the tests that drive the reader's
    /// own ends — the end of the control channel and the id-correlated delivery
    /// — rather than a spawned program. The extension path is empty, so nothing
    /// on disk is touched.
    fn reader_with_control(control: Arc<PiControl>) -> PiReader {
        let stdin: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(None));
        PiReader::new(
            Vec::new(),
            SessionEvent::SessionManifest {
                provider_id: Some("pi".to_string()),
                current_model_id: None,
                models: Vec::new(),
                modes: None,
            },
            super::PermissionBroker::for_test(Arc::new(|_, _| Ok(()))),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(AtomicU64::new(1)),
            control,
            stdin,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
        )
    }

    #[test]
    fn the_control_channel_ending_wakes_every_waiter() {
        // A2-02: the reader thread is what delivers answers, so when the child's
        // output ends no answer can arrive for anyone still waiting. Left
        // registered, each waiter would sit out the whole response timeout for a
        // reply the reader can no longer deliver — the steer's own timeout is
        // fifteen seconds of that.
        let stdin: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(None));
        let control = Arc::new(PiControl::new(
            Arc::clone(&stdin),
            Arc::new(AtomicU64::new(1)),
        ));
        let response = {
            // Registered exactly as `begin` registers one, without a child to
            // write to: what this pins is the wait.
            let (sender, receiver) = std::sync::mpsc::channel();
            control
                .pending
                .lock()
                .expect("pending")
                .insert("c-1".to_string(), sender);
            receiver
        };
        let mut reader = reader_with_control(Arc::clone(&control));
        let runtime = Arc::new(SessionRuntime::new());

        reader.finish(&runtime);

        let answer = response
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the waiter is woken by the end of the channel");
        let message = answer.expect_err("no answer can arrive: the channel is over");
        assert!(message.contains("control channel"), "{message}");
        assert!(
            control.pending.lock().expect("pending").is_empty(),
            "and the table is left with no waiter to wake twice"
        );
    }

    #[test]
    fn a_control_response_for_an_id_nobody_waits_for_is_ignored() {
        // A2-02: the id space carries answers that are not this control's — to
        // commands no waiter registered, and to requests whose waiter already
        // timed out. An unknown id is nothing to deliver, not an error and not a
        // panic, and it must not disturb the waiter that *is* registered.
        let stdin: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(None));
        let control = PiControl::new(Arc::clone(&stdin), Arc::new(AtomicU64::new(1)));
        let (sender, receiver) = std::sync::mpsc::channel();
        control
            .pending
            .lock()
            .expect("pending")
            .insert("c-1".to_string(), sender);

        assert!(
            !control.deliver(&serde_json::json!({
                "id": "c-404",
                "type": "response",
                "success": true,
            })),
            "no waiter holds that id"
        );
        assert!(
            !control.deliver(&serde_json::json!({ "type": "response", "success": true })),
            "a response with no id at all names nobody"
        );

        assert!(control.deliver(&serde_json::json!({
            "id": "c-1",
            "type": "response",
            "success": true,
        })));
        let answer = receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the registered waiter still takes its own answer");
        assert_eq!(answer.expect("an answer, not a failure")["id"], "c-1");
    }

    /// The refusal Pi sends for a command its build has no handler for, in the
    /// spelling the test passes (A2-11): the script is `FAKE_PI_UNKNOWN_STEER`
    /// with its error message replaced, so the two cases differ in nothing else.
    fn fake_pi_refusing(error: &str) -> String {
        FAKE_PI_UNKNOWN_STEER.replace("Unknown command: steer", error)
    }

    #[test]
    fn a_pi_steer_refusal_is_recognised_whatever_its_case() {
        // A2-11: what makes the steer unavailable is the app-server saying it
        // has no such command, not the exact spelling. A case-sensitive match
        // turns `unknown command: steer` into an `Err` — "the steer's fate is
        // unknown" — for a build that said exactly what happened.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        let pi = fake_pi_answering(&fake_pi_refusing("unknown command: steer"));
        let mut steerer = PiSteerer {
            control: Arc::clone(&pi.control),
        };
        assert!(
            matches!(
                steer_through_the_turn(&mut steerer, "turn left"),
                Some(Ok(false))
            ),
            "a lower-case refusal is the same refusal"
        );
        let answers = pi.answers();
        assert_eq!(answers.len(), 1, "one answer, read by the reader");
        assert_eq!(
            answers[0]["error"],
            serde_json::json!("unknown command: steer")
        );
    }

    #[test]
    fn a_pi_that_does_not_know_the_steer_command_is_unavailable_rather_than_steered() {
        // The answer Pi sends is the one that decides: a write alone reports
        // that the bytes reached the pipe, which a build without `steer` also
        // does before it rejects the command (the fix-pass rule for S4-01 on
        // this provider). The daemon has to read the answer, so the steer goes
        // through the id-correlated round-trip.
        // A2-13: the fake Pi is a `node` script, so this skips where there is no
        // node rather than failing there.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        let pi = fake_pi_answering(FAKE_PI_UNKNOWN_STEER);
        let mut steerer = PiSteerer {
            control: Arc::clone(&pi.control),
        };
        assert!(
            matches!(
                steer_through_the_turn(&mut steerer, "turn left"),
                Some(Ok(false))
            ),
            "an unknown command is unavailable, not steered"
        );
        let answers = pi.answers();
        assert_eq!(
            answers.len(),
            1,
            "one answer, read by the delivering reader"
        );
        assert_eq!(
            answers[0]["error"],
            serde_json::json!("Unknown command: steer"),
            "the refusal is what made the steer unavailable"
        );
    }

    #[test]
    fn a_pi_that_takes_the_steer_is_steered_with_the_frame_the_round_trip_wrote() {
        // A2-13: the fake Pi is a `node` script, so this skips where there is no
        // node rather than failing there.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        let pi = fake_pi_answering(FAKE_PI_STEERS);
        let mut steerer = PiSteerer {
            control: Arc::clone(&pi.control),
        };
        assert!(matches!(
            steer_through_the_turn(&mut steerer, "hello"),
            Some(Ok(true))
        ));
        let answers = pi.answers();
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0]["success"], serde_json::json!(true));
        // The frame as the child received it, byte for byte: `begin` mints the
        // id that correlates the answer, and the steer's own fields follow it.
        assert_eq!(
            answers[0]["received"].as_str(),
            Some(r#"{"id":"c-1","type":"steer","text":"hello","images":[]}"#)
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
    #[cfg(test)]
    mod delivery_tests {
        use super::{fake_pi_answering, PiCatalog, PiControl, PiSwitcher};
        use crate::session::ModelSwitcher;
        use devboule_protocol::SessionModelEffort;
        use std::collections::HashMap;
        use std::sync::atomic::{AtomicBool, AtomicU64};
        use std::sync::{Arc, Mutex};

        // Re-exported through the tests module's own namespace where they are
        // already in scope; the two that are not come straight from home.
        use crate::session::pi_client::{PiInputKinds, PiModel};

        /// A fake Pi that answers every control command with success and, for
        /// `get_available_thinking_levels`, a real level list. Each answer echoes
        /// the request line back, so the test can assert on the *requests* — the
        /// wire is what the delivery is.
        const FAKE_PI_DELIVERS: &str = r#"
    let buffered = "";
    process.stdin.on("data", (chunk) => {
      buffered += chunk;
      let index;
      while ((index = buffered.indexOf("\n")) >= 0) {
        const line = buffered.slice(0, index);
        buffered = buffered.slice(index + 1);
        const frame = JSON.parse(line);
        const answer = { id: frame.id, type: "response", success: true, received: line };
        if (frame.type === "get_available_thinking_levels") {
          answer.data = { levels: ["high", "low"] };
        }
        process.stdout.write(JSON.stringify(answer) + "\n");
      }
    });
    "#;

        /// The delivery is the wire: a delivered model and thinking option are
        /// the `set_model` and `set_thinking_level` requests this client writes
        /// after the handshake, in that order, with the delivered values. A
        /// delivery that skips a request is not a delivery.
        #[test]
        fn delivery_writes_set_model_and_set_thinking_level_on_the_wire() {
            let pi = fake_pi_answering(FAKE_PI_DELIVERS);
            let catalog = PiCatalog {
                models: HashMap::from([(
                    "pi-model".to_string(),
                    PiModel {
                        name: "Pi Model".to_string(),
                        provider: Some("pi-provider".to_string()),
                        context_tokens: None,
                        efforts: Some(vec![
                            SessionModelEffort {
                                id: "high".to_string(),
                                label: "High".to_string(),
                                description: None,
                                default: Some(true),
                            },
                            SessionModelEffort {
                                id: "low".to_string(),
                                label: "Low".to_string(),
                                description: None,
                                default: None,
                            },
                        ]),
                        input: PiInputKinds::default(),
                    },
                )]),
                current_model_id: Some("pi-model".to_string()),
                current_provider: Some("pi-provider".to_string()),
                current_effort: Some("high".to_string()),
                current_levels: vec!["high".to_string()],
            };
            let switcher = PiSwitcher {
                control: Arc::clone(&pi.control),
                catalog: Arc::new(Mutex::new(catalog)),
                mode_id: Arc::new(Mutex::new("ask".to_string())),
                permission_extension_active: Arc::new(AtomicBool::new(false)),
            };
            switcher
                .set_model(Some("pi-model"), Some("low"))
                .expect("delivered");
            let answers = pi.answers();

            let received: Vec<String> = answers
                .iter()
                .filter_map(|answer| answer["received"].as_str().map(|line| line.to_string()))
                .collect();
            let commands: Vec<String> = received
                .iter()
                .filter_map(|line| {
                    serde_json::from_str::<serde_json::Value>(line)
                        .ok()?
                        .get("type")
                        .and_then(|kind| kind.as_str())
                        .map(str::to_string)
                })
                .collect();
            assert_eq!(
                commands,
                vec![
                    "set_model".to_string(),
                    "get_available_thinking_levels".to_string(),
                    "set_thinking_level".to_string()
                ],
                "the delivery writes model then level, on the wire: {received:?}"
            );
            assert!(
                received[0].contains("pi-model"),
                "the set_model request carries the delivered model: {}",
                received[0]
            );
            assert!(
                received[2].contains("low"),
                "the set_thinking_level request carries the delivered level: {}",
                received[2]
            );
        }

        /// The model-absence refusal fires before anything is written: a provider
        /// that publishes no models cannot deliver any choice, and that is a
        /// different sentence from "the named model is not in the list". The
        /// control's stdin is closed — if the test reaches the wire it fails
        /// loudly instead of passing quietly.
        #[test]
        fn a_pi_model_against_an_empty_catalog_is_the_absence_refusal() {
            let stdin: Arc<Mutex<Option<std::process::ChildStdin>>> = Arc::new(Mutex::new(None));
            let switcher = PiSwitcher {
                control: Arc::new(PiControl::new(stdin, Arc::new(AtomicU64::new(1)))),
                catalog: Arc::new(Mutex::new(PiCatalog {
                    models: HashMap::new(),
                    current_model_id: None,
                    current_provider: None,
                    current_effort: None,
                    current_levels: Vec::new(),
                })),
                mode_id: Arc::new(Mutex::new("ask".to_string())),
                permission_extension_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            };
            let error = switcher
                .set_model(Some("pi-model"), None)
                .expect_err("an empty catalog cannot deliver a model");
            assert!(
                error.message.contains("publishes no models"),
                "the absence sentence: {}",
                error.message
            );
            assert!(
                !error.message.contains("is not in get_available_models"),
                "the two sentences must stay distinct: {}",
                error.message
            );
        }

        /// The tick contradiction, walked over pi's own closed mode
        /// vocabulary plus the broker table (the R2a audit's F9 — this
        /// refusal had no test at all): for every mode pi can start in, a
        /// tick is admitted exactly when the daemon's broker answers that
        /// mode, and refused with the contradiction sentence otherwise.
        #[test]
        fn a_pi_auto_accept_tick_over_an_asking_mode_is_refused() {
            fn delivery(mode: &str, tick: bool) -> crate::profile_delivery::ProfileDelivery {
                let mut delivery = crate::profile_delivery::ProfileDelivery::for_child(
                    mode,
                    "pi-model",
                    None,
                    &serde_json::Map::new(),
                );
                delivery.auto_accept = tick;
                delivery
            }
            let validate_delivery = super::super::validate_delivery;

            // The closed table intersects pi's own vocabulary at exactly
            // `bypass`: that intersection is the route a tick rides, so it
            // is asserted, not assumed.
            let broker = crate::provider_catalog::auto_answered_modes();
            assert!(
                broker.contains(&"bypass"),
                "bypass is the pi mode the daemon's broker answers"
            );
            let vocabulary = ["bypass", "ask"];
            for mode_id in vocabulary {
                if crate::provider_catalog::mode_is_auto_answered(mode_id) {
                    validate_delivery(&delivery(mode_id, true)).unwrap_or_else(|error| {
                        panic!("{mode_id} answers its own prompts: {}", error.message)
                    });
                } else {
                    let error = validate_delivery(&delivery(mode_id, true))
                        .expect_err("a tick over an asking mode is the contradiction");
                    assert!(
                        error.message.contains("contradict") && error.message.contains(mode_id),
                        "the refusal names both halves for {mode_id}: {}",
                        error.message
                    );
                }
                validate_delivery(&delivery(mode_id, false)).unwrap_or_else(|error| {
                    panic!("{mode_id} without the tick: {}", error.message)
                });
            }

            // A mode that is not pi's at all is refused with its own
            // sentence, tick or no tick.
            let error = validate_delivery(&delivery("default", false))
                .expect_err("an unknown mode is refused");
            assert!(
                error.message.contains("is not available"),
                "the mode sentence: {}",
                error.message
            );
        }
    }

    /// A fake Pi that logs every command it is asked and answers each with
    /// the id-correlated success the control protocol spells, with real
    /// levels for `get_available_thinking_levels`.
    const FAKE_PI_DELIVERY_ANSWERS: &str = r#"
const fs = require("fs");
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    fs.appendFileSync(process.env.DEVBOULE_FAKE_PI_LOG, frame.type + "\n");
    const answer = { id: frame.id, type: "response", success: true, received: line };
    if (frame.type === "get_available_thinking_levels") {
      answer.data = { levels: ["high", "low"] };
    }
    process.stdout.write(JSON.stringify(answer) + "\n");
  }
});
"#;

    /// The same fake, refusing every `set_model` the way a build that will
    /// not take the model answers: `success: false`, with an error.
    const FAKE_PI_DELIVERY_REFUSES: &str = r#"
const fs = require("fs");
// An open listener keeps this process alive when its stdin closes, so "the
// child exited" after the teardown can only mean the kill did it — a
// teardown that merely closed the pipe cannot satisfy the assertion (the
// re-audit's P3-6).
require("net").createServer().listen(0, "127.0.0.1");
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    fs.appendFileSync(process.env.DEVBOULE_FAKE_PI_LOG, frame.type + "\n");
    const answer = { id: frame.id, type: "response",
      success: frame.type !== "set_model",
      error: "unknown model", received: line };
    if (frame.type === "get_available_thinking_levels") {
      answer.data = { levels: ["high", "low"] };
    }
    process.stdout.write(JSON.stringify(answer) + "\n");
  }
});
"#;

    /// The gated fake: every request is logged the moment it arrives (so a
    /// test can see the delivery is in flight) but the `set_model` answer is
    /// held until a gate file appears. It is the P2-1 window made
    /// deterministic: the reader is live, the rpc is on the wire, the answer
    /// never comes.
    const FAKE_PI_DELIVERY_GATED: &str = r#"
const fs = require("fs");
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    fs.appendFileSync(process.env.DEVBOULE_FAKE_PI_LOG, frame.type + "\n");
    if (frame.type === "set_model" && !fs.existsSync(process.env.DEVBOULE_FAKE_PI_GATE)) {
      const held = setInterval(() => {
        if (fs.existsSync(process.env.DEVBOULE_FAKE_PI_GATE)) {
          clearInterval(held);
          answer(frame);
        }
      }, 20);
      return;
    }
    answer(frame);
  }
});
function answer(frame) {
  const answer = { id: frame.id, type: "response", success: true };
  if (frame.type === "get_available_thinking_levels") {
    answer.data = { levels: ["high", "low"] };
  }
  process.stdout.write(JSON.stringify(answer) + "\n");
}
"#;

    /// A fake that serves the real `spawn_process` handshake — `get_state`,
    /// `get_available_models`, `get_available_thinking_levels` — and then
    /// answers whatever else comes, logging every frame. This is the fake
    /// the spawn seam is tested with (the re-audit's P2-3).
    const FAKE_PI_SPAWN_HANDSHAKE: &str = r#"
const fs = require("fs");
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    fs.appendFileSync(process.env.DEVBOULE_FAKE_PI_LOG, frame.type + "\n");
    let answer = { success: true };
    if (frame.type === "get_state") {
      answer.data = { model: { id: "pi-model", provider: "pi-provider" }, thinkingLevel: "high" };
    } else if (frame.type === "get_available_models") {
      answer.data = { models: [
        { id: "pi-model", name: "Pi Model", provider: "pi-provider", thinkingLevelMap: { high: {}, low: {} } }
      ] };
    } else if (frame.type === "get_available_thinking_levels") {
      answer.data = { levels: ["high", "low"] };
    }
    answer.id = frame.id;
    answer.type = "response";
    process.stdout.write(JSON.stringify(answer) + "\n");
  }
});
"#;

    /// The lifecycle the R2a audit's F1 convicted: a profile delivery for pi
    /// is an awaited control rpc, and the only code that can deliver its
    /// answer is the session reader thread `start_spawned_session` starts.
    /// These tests drive that real ordering — the `SpawnedSession` is
    /// assembled the way `spawn_process` assembles it, the delivery travels
    /// as [`pending_pi_delivery`] packages it, and **no fixture starts a
    /// reader for the client**: the deliverer under test is the production
    /// one. A regression that runs the delivery before that reader exists
    /// cannot be answered here except by the fifteen-second timeout and the
    /// refusal that follows — which is exactly the production failure.
    mod lifecycle_tests {
        use super::super::PtyCommand;
        use super::super::{
            pending_pi_delivery, spawn_process, PiCatalog, PiControl, PiInputKinds, PiKiller,
            PiModel, PiReader, PiStaticPrompt, PiStderr, PiStdout, PiSwitcher, PiWriter,
        };
        use crate::process_tree::JobObject;
        use crate::profile_delivery::ProfileDelivery;
        use crate::server::ServerState;
        use crate::session::permission_broker::PermissionBroker;
        use crate::session::{start_spawned_session, SpawnedSession, StdioWaitableChild};
        use devboule_protocol::{OwnerId, SessionEvent, SessionModelEffort};
        use std::collections::HashMap;
        use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
        use std::sync::atomic::{AtomicBool, AtomicU64};
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        /// The delivery test's model, verbatim: one model, two levels.
        fn delivery_catalog() -> PiCatalog {
            PiCatalog {
                models: HashMap::from([(
                    "pi-model".to_string(),
                    PiModel {
                        name: "Pi Model".to_string(),
                        provider: Some("pi-provider".to_string()),
                        context_tokens: None,
                        efforts: Some(vec![
                            SessionModelEffort {
                                id: "high".to_string(),
                                label: "High".to_string(),
                                description: None,
                                default: Some(true),
                            },
                            SessionModelEffort {
                                id: "low".to_string(),
                                label: "Low".to_string(),
                                description: None,
                                default: None,
                            },
                        ]),
                        input: PiInputKinds::default(),
                    },
                )]),
                current_model_id: Some("pi-model".to_string()),
                current_provider: Some("pi-provider".to_string()),
                current_effort: Some("high".to_string()),
                current_levels: vec!["high".to_string()],
            }
        }

        fn pi_delivery() -> ProfileDelivery {
            ProfileDelivery::for_child("ask", "pi-model", Some("low"), &serde_json::Map::new())
        }

        /// A fake Pi on real pipes. Stdout is deliberately **not** wrapped in
        /// a reader thread here — the `PiStdout` the session reader will
        /// drain is built inside `spawned_session`, and nothing else reads
        /// the child.
        struct SpawnedPi {
            process: Arc<Mutex<Child>>,
            stdout: ChildStdout,
            stdin: Arc<Mutex<Option<ChildStdin>>>,
            stderr: ChildStderr,
            next_id: Arc<AtomicU64>,
            catalog: Arc<Mutex<PiCatalog>>,
        }

        fn spawned_pi(script: &str, log: &std::path::Path) -> SpawnedPi {
            spawned_pi_with_env(script, log, &[])
        }

        fn spawned_pi_with_env(
            script: &str,
            log: &std::path::Path,
            extra: &[(&str, String)],
        ) -> SpawnedPi {
            let mut child = Command::new("node")
                .arg("-e")
                .arg(script)
                .env("DEVBOULE_FAKE_PI_LOG", log)
                .envs(extra.iter().map(|(key, value)| (*key, value)))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap_or_else(|error| {
                    panic!(
                        "{}",
                        super::node_unavailable("Pi delivery lifecycle test", &error)
                    )
                });
            let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
            let stdout = child.stdout.take().expect("stdout");
            let stderr = child.stderr.take().expect("stderr");
            SpawnedPi {
                process: Arc::new(Mutex::new(child)),
                stdout,
                stdin,
                stderr,
                next_id: Arc::new(AtomicU64::new(1)),
                catalog: Arc::new(Mutex::new(delivery_catalog())),
            }
        }

        /// The `SpawnedSession` the production spawn assembles, with the
        /// delivery pending exactly as `pending_pi_delivery` packages it. No
        /// reader thread is started here: `start_spawned_session` is what
        /// starts it, which is the fact under test.
        fn spawned_session(pi: SpawnedPi) -> SpawnedSession {
            let stdout = PiStdout::spawn(pi.stdout).expect("Pi stdout");
            let control = Arc::new(PiControl::new(
                Arc::clone(&pi.stdin),
                Arc::clone(&pi.next_id),
            ));
            let permission_broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
            let reader = PiReader::new(
                Vec::new(),
                SessionEvent::SessionManifest {
                    provider_id: Some("pi".to_string()),
                    current_model_id: None,
                    models: Vec::new(),
                    modes: None,
                },
                Arc::clone(&permission_broker),
                Arc::new(Mutex::new(HashMap::new())),
                Arc::clone(&pi.next_id),
                Arc::clone(&control),
                Arc::clone(&pi.stdin),
                Arc::new(AtomicBool::new(true)),
            );
            let switcher = PiSwitcher {
                control,
                catalog: Arc::clone(&pi.catalog),
                mode_id: Arc::new(Mutex::new("ask".to_string())),
                permission_extension_active: Arc::new(AtomicBool::new(true)),
            };
            let pending_delivery = pending_pi_delivery(&switcher, &pi_delivery());
            SpawnedSession {
                process_job: JobObject::new().expect("process job"),
                master: None,
                killer: Box::new(PiKiller {
                    process: Arc::clone(&pi.process),
                    stdin: Arc::clone(&pi.stdin),
                    next_id: Arc::clone(&pi.next_id),
                    permission_broker: Arc::clone(&permission_broker),
                    cancelled: Arc::new(AtomicBool::new(false)),
                    extension_path: std::env::temp_dir().join(format!(
                        "devboule-pi-lifecycle-ext-{}.ts",
                        std::process::id()
                    )),
                    bridge_path: None,
                }),
                switcher: Some(Box::new(switcher)),
                child: Box::new(StdioWaitableChild {
                    process: Arc::clone(&pi.process),
                }),
                writer: Arc::new(Mutex::new(Box::new(PiWriter {
                    stdin: Arc::clone(&pi.stdin),
                    next_id: Arc::clone(&pi.next_id),
                    pending: Vec::new(),
                })
                    as Box<dyn std::io::Write + Send>)),
                image_sink: None,
                static_image_sink: Some(Arc::new(PiStaticPrompt::new(
                    Arc::clone(&pi.stdin),
                    Arc::clone(&pi.next_id),
                    Arc::clone(&pi.catalog),
                ))),
                reader: Box::new(stdout),
                reader_dispatch: Some(Box::new(reader)),
                stderr: Some(Box::new(
                    PiStderr::start(pi.stderr).expect("stderr wrapper"),
                )),
                permission_broker: Some(Arc::clone(&permission_broker)),
                os_handle: None,
                peer_session_id: None,
                agent_version: None,
                pending_delivery,
                pending_codex_verify: None,
            }
        }

        fn metadata(id: &str) -> devboule_protocol::Session {
            devboule_protocol::Session {
                id: id.to_string(),
                workspace_id: None,
                cwd: None,
                kind: crate::session::SessionKind::Pi,
                title: "Pi".to_string(),
                state: devboule_protocol::SessionState::Live { generation: 1 },
                elapsed_ms: Some(0),
                provider: Some("pi".to_string()),
                peer_session_id: None,
                created_at_ms: 1,
                origin: devboule_protocol::SessionOrigin::local(),
                display_name: None,
                created_by: None,
                profile_id: None,
                context_id: None,
                unattended: devboule_protocol::UnattendedState::No,
                labels: Default::default(),
            }
        }

        fn log_path(tag: &str) -> std::path::PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "devboule-pi-lifecycle-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::create_dir_all(&dir);
            let log = dir.join("commands.log");
            let _ = std::fs::remove_file(&log);
            log
        }

        fn read_log(log: &std::path::Path) -> Vec<String> {
            std::fs::read_to_string(log)
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }

        fn wait_for_commands(log: &std::path::Path, wanted: &[&str]) -> Vec<String> {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let commands = read_log(log);
                if wanted
                    .iter()
                    .all(|want| commands.iter().any(|command| command == want))
                {
                    return commands;
                }
                assert!(
                    Instant::now() < deadline,
                    "the switch never reached the child: {commands:?}"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        /// Production never spawns a bare program name: the catalog resolves
        /// the provider executable to an absolute path before the launch
        /// (`InstalledAgent::acp_command` — element 0 is the path
        /// CreateProcess will run). The test resolves the same way, both to
        /// stay faithful to that contract and because a `current_dir` on the
        /// command changes how a bare name would be searched.
        fn node_program() -> String {
            let path = std::env::var_os("PATH").unwrap_or_default();
            for dir in std::env::split_paths(&path) {
                let candidate = dir.join("node.exe");
                if candidate.is_file() {
                    return candidate.to_string_lossy().into_owned();
                }
            }
            let error = std::io::Error::new(std::io::ErrorKind::NotFound, "node.exe not on PATH");
            panic!(
                "{}",
                super::node_unavailable("Pi spawn wiring test", &error)
            );
        }

        fn child_exited(process: &Arc<Mutex<Child>>) -> bool {
            for _ in 0..100 {
                if let Ok(mut child) = process.lock() {
                    if child.try_wait().ok().flatten().is_some() {
                        return true;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        }

        /// The answer to F1: the delivery completes because the session
        /// reader — the thread `start_spawned_session` starts, and nothing
        /// else — delivers the switch's answer. The three requests the
        /// switcher writes are on the wire and answered by the time the
        /// start returns.
        #[test]
        fn the_session_reader_delivers_the_profile_switch_start_spawned_session_awaits_it() {
            let log = log_path("ok");
            let state = ServerState::new("pi-delivery-lifecycle".to_string());
            let owner = OwnerId::new("local", "test").expect("owner");
            let pi = spawned_pi(super::FAKE_PI_DELIVERY_ANSWERS, &log);
            let session_id = "pifelifecycleok1".to_string();

            start_spawned_session(
                &state,
                &state.sessions,
                metadata(&session_id),
                owner.clone(),
                None,
                Some("ask".to_string()),
                spawned_session(pi),
                None,
            )
            .expect("the delivery completes once the session reader is live");

            let commands = wait_for_commands(
                &log,
                &[
                    "set_model",
                    "get_available_thinking_levels",
                    "set_thinking_level",
                ],
            );
            assert!(
                commands.contains(&"set_model".to_string()),
                "the switch is on the wire: {commands:?}"
            );
            let _ = state.sessions.close(&session_id, &owner, &None);
            let _ = std::fs::remove_dir_all(log.parent().expect("log dir"));
        }

        /// The other half of the repair: a refused switch is still a
        /// refusal — the hook's error tears the child down and fails the
        /// session start, so no child the card did not describe survives to
        /// answer anything, and the roster never lists it.
        #[test]
        fn a_refused_delivery_tears_the_child_down_and_fails_the_start() {
            let log = log_path("refused");
            let state = ServerState::new("pi-delivery-refusal".to_string());
            let owner = OwnerId::new("local", "test").expect("owner");
            let pi = spawned_pi(super::FAKE_PI_DELIVERY_REFUSES, &log);
            let process = Arc::clone(&pi.process);
            let session_id = "pifelifecycleref1".to_string();

            let error = start_spawned_session(
                &state,
                &state.sessions,
                metadata(&session_id),
                owner.clone(),
                None,
                Some("ask".to_string()),
                spawned_session(pi),
                None,
            )
            .expect_err("a refused switch must refuse the session start");

            assert!(
                error.message.contains("set_model failed"),
                "the refusal names the refused rpc: {}",
                error.message
            );
            assert!(
                !state
                    .sessions
                    .list(&owner)
                    .expect("roster")
                    .iter()
                    .any(|session| session.id == session_id),
                "the refused child is not left on the roster"
            );
            assert!(
                child_exited(&process),
                "the child was killed by the refusal teardown"
            );
            let _ = std::fs::remove_dir_all(log.parent().expect("log dir"));
        }

        /// The re-audit's P2-1: the session is not visible until it is
        /// configured. The gated fake holds the `set_model` answer, so the
        /// delivery — and with it the whole create — sits in flight while
        /// the child is already spawned and the reader already running. In
        /// that window the registry holds the entry as `Configuring`: no
        /// roster read may hand the id out, because a prompt sent now would
        /// be silently discarded if the delivery were refused. The old
        /// insert-as-live shape fails the not-listed assertion here.
        #[test]
        fn a_session_is_not_listed_until_its_delivery_lands() {
            let log = log_path("window");
            let gate = log.parent().expect("log dir").join("gate.txt");
            let _ = std::fs::remove_file(&gate);
            let state = ServerState::new("pi-delivery-window".to_string());
            let owner = OwnerId::new("local", "test").expect("owner");
            let pi = spawned_pi_with_env(
                super::FAKE_PI_DELIVERY_GATED,
                &log,
                &[("DEVBOULE_FAKE_PI_GATE", gate.to_string_lossy().into_owned())],
            );
            let session_id = "pifelifecyclewin1".to_string();
            let metadata = metadata(&session_id);
            let owner_for_start = owner.clone();
            let state_for_start = Arc::clone(&state);
            let start = std::thread::Builder::new()
                .name("pi-delivery-window-start".to_string())
                .spawn(move || {
                    start_spawned_session(
                        &state_for_start,
                        &state_for_start.sessions,
                        metadata,
                        owner_for_start,
                        None,
                        Some("ask".to_string()),
                        spawned_session(pi),
                        None,
                    )
                    .expect("the delivery completes once the gate opens")
                })
                .expect("spawn the create thread");

            // The request is on the wire and the fake is holding its answer:
            // the delivery — and therefore the create — is in flight now.
            wait_for_commands(&log, &["set_model"]);
            for _ in 0..10 {
                assert!(
                    !state
                        .sessions
                        .list(&owner)
                        .expect("roster")
                        .iter()
                        .any(|session| session.id == session_id),
                    "a session inside its delivery window is not listed"
                );
                assert!(
                    !state
                        .sessions
                        .state_snapshots(&owner)
                        .iter()
                        .any(|snapshot| snapshot.id == session_id),
                    "a session inside its delivery window has no snapshot"
                );
                // The repair pass's P1-1, reconciled with the one-door
                // design: the delete here is refused `SessionNotFound` —
                // the answer `peer_entry` gives every id-addressed peer
                // call for a `Configuring` entry — not the close-first
                // refusal. The decision, so the next reader does not
                // relitigate it:
                // - it is what the variant's own contract says — a
                //   `Configuring` entry is invisible to every id-addressed
                //   peer call;
                // - it does not confirm to a peer that the id exists, and
                //   during the window no peer legitimately holds that id
                //   (the create returns it only after promotion);
                // - an API where `sessions_list` says the session does not
                //   exist while `delete` says "close it first" contradicts
                //   itself. One door, one answer.
                // What the refusal must still prevent is the P1's harm: the
                // windowed entry holds a running child, and an unrefused
                // delete here would remove that child's entry mid-delivery.
                // Non-removal is proved end to end below: the gate opens,
                // the create completes, the id is listed.
                let delete_error = state
                    .sessions
                    .delete_session(&session_id, &owner)
                    .expect_err("delete inside the delivery window must be refused");
                assert_eq!(
                    delete_error.code,
                    devboule_protocol::ErrorCode::SessionNotFound,
                    "the windowed delete takes the peer-visibility door's answer, not the close-first guard: {delete_error:?}"
                );
                std::thread::sleep(Duration::from_millis(50));
            }

            // The gate opens, the delivery lands, the create returns — and
            // only then does the session exist for its peers. Resume's guard
            // (the re-audit's P2-1) sits behind the journal-row lookup and
            // `resume_handle`, and only ACP rows take that path — so the
            // resume refusal is observed by writing the row a resumed ACP
            // child carries and naming the windowed id the way the audit's
            // trigger describes. The named-provider resolution the resume
            // performs before the guard needs the direct-command override.
            std::env::set_var("DEVBOULE_ACP_COMMAND", r#"["cmd"]"#);
            std::env::set_var("DEVBOULE_ACP_PROVIDER_ID", "devboule-acp-stub");
            if let Some(journal) = state.sessions.journal.as_ref() {
                let mut row = crate::journal::new_session_record(
                    session_id.clone(),
                    owner.user.clone(),
                    None,
                    devboule_protocol::SessionKind::Acp,
                    "gated delivery",
                );
                row.provider = Some("devboule-acp-stub".to_string());
                row.peer_session_id = Some("stub-session".to_string());
                journal
                    .upsert_blocking(row)
                    .expect("the resumed row is on the journal");
            }
            let conn = crate::session::ConnHandle::new(91);
            let resume_error = state
                .sessions
                .resume(&state, &session_id, &owner, &conn)
                .expect_err("resume inside the delivery window must be refused");
            assert!(
                resume_error
                    .message
                    .contains("cannot be resumed while its process is running"),
                "the resume refusal names the running child: {resume_error:?}"
            );
            std::env::remove_var("DEVBOULE_ACP_COMMAND");
            std::env::remove_var("DEVBOULE_ACP_PROVIDER_ID");
            // Still refused, still present: the guard is a refusal, not a
            // teardown, and the child the create will return is untouched.
            assert!(
                !state
                    .sessions
                    .list(&owner)
                    .expect("roster")
                    .iter()
                    .any(|session| session.id == session_id),
                "the windowed child stays hidden after the refused resume"
            );

            std::fs::write(&gate, b"go").expect("open the gate");
            start.join().expect("the create thread");
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let listed = state
                    .sessions
                    .list(&owner)
                    .expect("roster")
                    .iter()
                    .any(|session| session.id == session_id);
                assert!(
                    listed || Instant::now() < deadline,
                    "the session is listed once its delivery has landed"
                );
                if listed {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            // The close-first arm must not go silently dead above the
            // window: the same id, promoted, is a peer-visible session
            // holding a running child, and its delete is refused with the
            // close-first refusal — the arm the `SessionNotFound`
            // reconciliation answers around, not removes. A suite that lost
            // this assertion would delete the guard by unreachability.
            let close_first_error = state
                .sessions
                .delete_session(&session_id, &owner)
                .expect_err("deleting a live session must be refused");
            assert_eq!(
                close_first_error.code,
                devboule_protocol::ErrorCode::InvalidRequest,
                "the live delete is the close-first refusal: {close_first_error:?}"
            );
            assert_eq!(
                close_first_error.message, "Close the session before deleting it.",
                "the live delete refusal demands the close"
            );
            assert!(
                state
                    .sessions
                    .list(&owner)
                    .expect("roster")
                    .iter()
                    .any(|session| session.id == session_id),
                "the refused live delete leaves the session listed"
            );
            let _ = state.sessions.close(&session_id, &owner, &None);
            let _ = std::fs::remove_dir_all(log.parent().expect("log dir"));
        }

        /// The re-audit's P2-3: the seam the F1 repair created is
        /// `spawn_process` building the hook and handing it to the
        /// `SpawnedSession` — and no test called `spawn_process` for pi at
        /// all, so passing `None` there left the suite green while the
        /// profile's model died silently at spawn. This test runs the real
        /// seam: the real `spawn_process` (its handshake served by the fake,
        /// its extension written into the state's runtime dir), its output
        /// fed to the real `start_spawned_session`, and the profile's switch
        /// asserted on the child's wire. Cut the wiring — `None` in place of
        /// the hook — and nothing ever answers the switch: the wait times
        /// out, red.
        #[test]
        fn spawn_process_wires_the_delivery_the_reader_will_run() {
            let log = log_path("spawnwiring");
            let state = ServerState::new("pi-spawn-wiring".to_string());
            let owner = OwnerId::new("local", "test").expect("owner");
            // The fake is a script FILE the way a real Pi launch carries its
            // entry, with `--` ending the node options: `spawn_args` injects
            // `--mode rpc` and the extension path after the entry, and those
            // are the entry's argv, not node's.
            let script = log.parent().expect("log dir").join("fake-pi-entry.js");
            std::fs::write(&script, super::FAKE_PI_SPAWN_HANDSHAKE)
                .expect("write the fake pi entry");
            let command = PtyCommand::new(
                node_program(),
                vec![script.to_string_lossy().into_owned(), "--".to_string()],
                std::env::temp_dir(),
                vec![(
                    "DEVBOULE_FAKE_PI_LOG".to_string(),
                    log.to_string_lossy().into_owned(),
                )],
            );
            let delivery = ProfileDelivery::for_child(
                "bypass",
                "pi-model",
                Some("low"),
                &serde_json::Map::new(),
            );
            let spawned = spawn_process(&state, command, None, delivery)
                .expect("spawn_process assembles the child and the delivery hook");
            let session_id = "pifelifecyclespawn1".to_string();
            start_spawned_session(
                &state,
                &state.sessions,
                metadata(&session_id),
                owner.clone(),
                None,
                None,
                spawned,
                None,
            )
            .expect("the delivery lands once the session reader is live");
            let commands = wait_for_commands(
                &log,
                &[
                    "set_model",
                    "get_available_thinking_levels",
                    "set_thinking_level",
                ],
            );
            assert!(
                commands.contains(&"set_model".to_string()),
                "the profile's switch reached the child through production's own wiring: {commands:?}"
            );
            let _ = state.sessions.close(&session_id, &owner, &None);
            let _ = std::fs::remove_dir_all(log.parent().expect("log dir"));
        }
    }
}
