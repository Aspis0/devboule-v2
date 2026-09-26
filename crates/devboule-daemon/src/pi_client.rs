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

use super::permission_broker::{PermissionBroker, PermissionResponseError, PermissionSender};
use super::PtyCommand;
use super::{
    write_child_stdin, ModelSwitcher, OutOfBandCommands, ReaderDispatch, SessionKiller,
    SessionRuntime, SessionSteerer, SpawnedSession, StderrSource, StdioWaitableChild, TurnToken,
};
use crate::acp_view::PromptCapabilityState;
use crate::atomic::atomic_write;
use crate::paths::RuntimePaths;
use crate::process_tree::{JobObject, ProcessHandle};
use crate::profile_delivery::ProfileDelivery;
use crate::server::ServerState;

/// Pi's `get_commands` list: the request the spawn writes and the waiter
/// behind it. A child of this file — it needs the control channel that
/// lives here, and its tests drive this client's reader.
#[path = "pi_commands.rs"]
mod commands;
/// The two commands pi executes itself, out of band. A child of this file
/// for the same control channel; its tests drive this client's writer.
#[path = "pi_out_of_band.rs"]
mod out_of_band;

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
    // Devices: the paired-device discovery read, answered from the daemon's
    // own rows; the calling session's own user scopes it, never an argument.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_LIST_DEVICES_TOOL,
        requires_confirmation: false,
    },
    // Peer agents: the one-dial roster read; one call names one device and
    // the responder scopes the answer to the pairing user, never an argument.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL,
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
    // Activity: one live agent's derived state plus its recent kinds, metadata
    // only and never transcript text; the broker judges peer callers at the
    // origin door before anything is touched.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_ACTIVITY_TOOL,
        requires_confirmation: false,
    },
    // Stop: kills one of the caller's own children's process trees; the row
    // and its transcript stay. Destructive, so the origin door judges it —
    // and refuses every peer — before anything is touched; a generic confirm
    // here would not know which child the card was about.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_STOP_AGENT_TOOL,
        requires_confirmation: false,
    },
    // Close: ends one of the caller's own children; the transcript stays in
    // history. Same door, same every-peer refusal, same reasoning as the
    // stop tool.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_CLOSE_AGENT_TOOL,
        requires_confirmation: false,
    },
    // Cancel: interrupts one of the caller's own children's turns and keeps
    // the child; the origin door judges it as `SessionInterrupt` — the
    // administrative capability — before anything is touched, so the generic
    // confirm would only re-ask a fact the door already decided.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_CANCEL_AGENT_TOOL,
        requires_confirmation: false,
    },
    // Pending list: the caller's own children's parked cards — a read beside
    // the roster, and answering still runs the answer tool's own checks.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_LIST_PENDING_PERMISSIONS_TOOL,
        requires_confirmation: false,
    },
    // Status: one caller's own child's snapshot — a read like the roster,
    // and its closed-child fallback reads only rows `created_by` the caller.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_GET_AGENT_STATUS_TOOL,
        requires_confirmation: false,
    },
    // Project graph: read-only queries of the graph the indexer wrote for the
    // caller's own workspace. They add nothing a local agent cannot already
    // read (the files are in its cwd), so no generic confirm would have
    // anything to ask about; the origin door refuses every peer.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_NEIGHBORHOOD_TOOL,
        requires_confirmation: false,
    },
    PiToolPolicy {
        name: crate::provider_catalog::MCP_IMPORTS_TOOL,
        requires_confirmation: false,
    },
    PiToolPolicy {
        name: crate::provider_catalog::MCP_IMPORTERS_TOOL,
        requires_confirmation: false,
    },
    // Oracle search: read-only semantic search of the caller's own workspace,
    // answered by the desktop app's engine through the daemon's forward leg.
    // The origin door judges peers before the forward leg runs and the broker
    // owns every refusal sentence, so no generic confirm would have anything
    // to ask that the door and the phrases do not already say.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_ORACLE_SEARCH_TOOL,
        requires_confirmation: false,
    },
    // Workspaces: the inventory read of the caller's own project, scoped
    // through its session row; a peer's read is the wire inventory read
    // under the administrative capability, judged at the origin door.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_LIST_WORKSPACES_TOOL,
        requires_confirmation: false,
    },
    // Create workspace: consented by the broker's own first-use card, which
    // carries the project facts; a generic confirm here would be a second
    // card with none of them.
    PiToolPolicy {
        name: crate::provider_catalog::MCP_CREATE_WORKSPACE_TOOL,
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
/// `pi.registerTool` per broker tool (twenty today), closed schemas matching the
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
    name: "devboule_list_devices",
    label: "List paired Devboule devices",
    description: `Lists the devices this machine is paired with: each device's id (the name to use when referring to it), its display name, its role, and whether it currently has a live connection to this machine. Answers locally, without contacting the other devices.`,
    parameters: Type.Object({}, { additionalProperties: false }),
    async execute(_toolCallId, _params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_list_devices", arguments: {} }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_list_peer_agents",
    label: "List agents on a paired device",
    description: `Lists the agents running right now on one paired device, named by deviceId from devboule_list_devices. Each agent is identified by the pair of device id and session id, and carries its name, provider, model, state and creation depth. This is what is live on that device at the moment of the call, not a stored list, and the device is dialled once per call; a cold connection can take several seconds.`,
    parameters: Type.Object(
      {
        deviceId: Type.String({ description: "The id of one paired device, from devboule_list_devices." }),
      },
      { required: ["deviceId"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_list_peer_agents", arguments: params }, signal);
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
    description: `Creates a new Devboule agent session from a profile the human enabled for agents, and sends it an initial prompt. The human is asked to authorize the first creation from this session; the result is the new session's id, its A2A task and context, and its display name. With notifyOnFinish false the child is also exempt from the idle (quiet) notice.`,
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

  pi.registerTool({
    name: "devboule_agent_activity",
    label: "Read Devboule agent activity",
    description: `Reads what one live agent session of your own owner has been doing: its current activity (working, idle, blocked or unknown), how long since it last published, and its recent event kinds with timestamps. Metadata only, never transcript text. Name the session by id or display name; limit caps the recent lines (default 10, max 50; 0 returns the state with no recent lines).`,
    parameters: Type.Object(
      {
        session: Type.String({ description: "The id or display name of one live agent session of your own owner." }),
        limit: Type.Optional(Type.Integer({ minimum: 0, maximum: 50, description: "How many recent event lines to return. Default 10, max 50." })),
      },
      { required: ["session"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_agent_activity", arguments: params }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_stop_agent",
    label: "Stop Devboule agent",
    description: `Stops one of your own live child sessions: its process tree is killed and the child stops running, while its session row and transcript stay in history. Use this for a child that is stuck or that you no longer need running. Name the child by id or display name; you can only stop a session you created yourself.`,
    parameters: Type.Object(
      {
        session: Type.String({ description: "The id or display name of one of your own live child sessions." }),
      },
      { required: ["session"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_stop_agent", arguments: params }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_close_agent",
    label: "Close Devboule agent",
    description: `Ends one of your own live child sessions: the live session goes away and its transcript stays in history. Use this to finish with a child you created and no longer need. Name the child by id or display name; you can only close a session you created yourself.`,
    parameters: Type.Object(
      {
        session: Type.String({ description: "The id or display name of one of your own live child sessions." }),
      },
      { required: ["session"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_close_agent", arguments: params }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_cancel_agent",
    label: "Cancel Devboule agent turn",
    description: `Interrupts the current turn of one of your own live child sessions and keeps the child: the child stops what it is doing now, any permission card it had parked is resolved as interrupted, and it stays alive for your next message. This is the soft verb between doing nothing and devboule_stop_agent, which kills the process. Name the child by id or display name; you can only cancel a session you created yourself. Replies success: true when a turn was interrupted, success: false when the child had no turn running - nothing was interrupted, and that is not an error.`,
    parameters: Type.Object(
      {
        agentId: Type.String({ description: "The id or display name of one of your own live child sessions." }),
      },
      { required: ["agentId"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_cancel_agent", arguments: params }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_list_pending_permissions",
    label: "List Devboule pending permissions",
    description: `Lists the permission cards your own live children are parked on right now, whatever the human's delegation switch says: each card's agentId, cardId, title, kind and a short excerpt of what the child asked. Listing is a read of your own children only; answering a card still requires the human's delegation switch and goes through devboule_answer_permission. A child with no cards adds no entry, and an empty list means nothing is parked.`,
    parameters: Type.Object({}, { additionalProperties: false }),
    async execute(_toolCallId, _params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_list_pending_permissions", arguments: {} }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_get_agent_status",
    label: "Read Devboule agent status",
    description: `Reads one of your own children as a snapshot: its state (a parked card shows as input_required), provider, model, mode, profile, who created it, its depth, how long since it last published, and the permission cards it is parked on. Name the child by id or display name; you can only read a session you created yourself. A child that has been closed answers from its stored row with no pending permissions, and anything else - a sibling, a stranger's session, an invented id - reads as not found.`,
    parameters: Type.Object(
      {
        agentId: Type.String({ description: "The id or display name of one of your own live child sessions." }),
      },
      { required: ["agentId"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_get_agent_status", arguments: params }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_project_neighborhood",
    label: "Read Devboule project graph",
    description: `Walks the project's code-knowledge graph from one node and answers the nodes reachable within a number of edges, each with its shortest distance from the node you named. The graph belongs to the calling session's own workspace: the indexer builds it from that folder's files, and a node id is a repository-relative path (a file) or that path with a '#start-end-index' suffix (a symbol inside it). depth is 1 to 4 edges (default 1); kind filters on the graph's two edge kinds, IMPORT and CONTAIN. Topology only: no source text, no symbol bodies, no semantic search. A node the graph does not contain answers with an empty list, exactly like a node with no edges. Fails when the session has no workspace, and when that workspace has no graph yet - never by reading another project's graph.`,
    parameters: Type.Object(
      {
        node: Type.String({ description: "The node to start from: a repository-relative file path, or that path with a '#start-end-index' suffix for a symbol." }),
        depth: Type.Optional(Type.Integer({ minimum: 1, maximum: 4, description: "How many edges to walk, 1 to 4. Default 1." })),
        kind: Type.Optional(Type.Union([Type.Literal("IMPORT"), Type.Literal("CONTAIN")], { description: "Restrict the walk to one edge kind. Omit for both." })),
      },
      { required: ["node"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_project_neighborhood", arguments: params }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_project_imports",
    label: "Read Devboule project imports",
    description: `Answers which files one file imports, from the project's code-knowledge graph in the calling session's own workspace. file is named by its repository-relative path as the graph spells it; a symbol id names a symbol rather than a file and answers with an empty list, as does a path the graph does not know. Import edges only: the file-to-file dependencies the indexer resolved inside the indexed set, never a guess at the filesystem. For the reverse direction use devboule_project_importers; this tool answers no source text and no semantic search.`,
    parameters: Type.Object(
      {
        file: Type.String({ description: "A repository-relative file path as the graph spells it." }),
      },
      { required: ["file"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_project_imports", arguments: params }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_project_importers",
    label: "Read Devboule project importers",
    description: `Answers which files import one file - the reverse of devboule_project_imports, read from the project's code-knowledge graph in the calling session's own workspace. file is named by its repository-relative path as the graph spells it. Import edges only, never call edges: 'who calls this function' is a question this graph cannot answer, and this tool does not answer it with an empty list that would look like 'nobody does'.`,
    parameters: Type.Object(
      {
        file: Type.String({ description: "A repository-relative file path as the graph spells it." }),
      },
      { required: ["file"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_project_importers", arguments: params }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_oracle_search",
    label: "Search Devboule Oracle",
    description: `Answers questions about the code of the calling session's own workspace by meaning, not by keyword: the Oracle index built for that folder is searched and the closest chunks come back as citations. query is the question in natural language; limit is how many chunks to return (1 to 10, default 10). Each result carries a repository-relative path, a line range, a narrower focus span when one was scored, the chunk text, a score that is a rank fusion (RRF) rather than a cosine similarity, and whether the chunk was found densely, lexically, or both. The folder searched is the calling session's own workspace, taken from the session's row and never from an argument; a session with no workspace is refused. This tool needs the Devboule desktop app running: the engine, the index and the local models live in the app, so with the app closed the call fails with a sentence that says exactly that. The project-graph tools (devboule_project_neighborhood, devboule_project_imports, devboule_project_importers) do not need the app. Fail-closed: no index, no model, no vectors, or a model still loading each answers with its own reason and the action to take - never an empty result list standing in for a missing fact, and never an answer from another project's index.`,
    parameters: Type.Object(
      {
        query: Type.String({ description: "The question to answer by meaning, in natural language." }),
        limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 10, description: "How many chunks to return, 1 to 10. Default 10." })),
      },
      { required: ["query"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_oracle_search", arguments: params }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_list_workspaces",
    label: "List Devboule workspaces",
    description: `Lists the workspaces of the calling session's own project: each workspace's id, name, checkout path, kind and branch. Only the caller's project is ever listed, and the project comes from the session's row, never from an argument.`,
    parameters: Type.Object({}, { additionalProperties: false }),
    async execute(_toolCallId, _params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_list_workspaces", arguments: {} }, signal);
      return { content: result?.content ?? [], details: result ?? {} };
    },
  });

  pi.registerTool({
    name: "devboule_create_workspace",
    label: "Create Devboule workspace",
    description: `Creates a workspace inside the calling session's own project, as the project folder itself or a new git worktree beside it, and answers the new workspace record. The human is asked to approve workspace writes from this session the first time. branch names the worktree branch and is worktree-only; name sets the workspace title. No path is accepted: the checkout is the project folder or a sibling worktree of it, never an agent-named directory.`,
    parameters: Type.Object(
      {
        isolation: Type.Union([Type.Literal("local"), Type.Literal("worktree")], { description: "The workspace shape: the project folder itself, or a new git worktree beside it." }),
        name: Type.Optional(Type.String({ description: "The workspace title." })),
        branch: Type.Optional(Type.String({ description: "The worktree branch. Worktree only." })),
        path: Type.Optional(Type.String({ description: "Not accepted: workspaces are created inside your project, never at an agent-named directory." })),
        projectId: Type.Optional(Type.String({ description: "Must be your own project, when given." })),
      },
      { required: ["isolation"], additionalProperties: false },
    ),
    async execute(_toolCallId, params, signal) {
      await brokerSession(signal);
      const result = await mcpRequest("tools/call", { name: "devboule_create_workspace", arguments: params }, signal);
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
            (rpc, agent.spawn_path_env)
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
    Ok(
        PtyCommand::new(program, argv, cwd, spawn_path_env.into_iter().collect())
            .with_provider_id("pi"),
    )
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

/// The resume half of the launch argv: `--session <peer>` spliced where
/// `spawn_args` leaves Pi's options, before any `--`, so a caller argv that
/// ends option parsing still carries it as the option it is. The id is the
/// `sessionId` the wire reported; Pi resolves it against its session
/// directory for this cwd first, then across projects.
fn spawn_resume_args(
    command: &PtyCommand,
    permission_path: &Path,
    bridge_path: Option<&Path>,
    peer_session_id: &str,
) -> Result<Vec<String>, WireError> {
    let mut args = spawn_args(command, permission_path, bridge_path)?;
    let index = args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len());
    args.splice(
        index..index,
        ["--session".to_string(), peer_session_id.to_string()],
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
    // The declaration says pi offers the tick and nothing else, and this is
    // where that is enforced: a stored feature beyond the tick has no frame in
    // this family (the mode and the injected extension are its whole surface),
    // so it is refused rather than left off the child's command line.
    crate::profile_delivery::refuse_undeclared(
        &crate::provider_features::pi_declarations(),
        delivery.model_id.as_deref(),
        &delivery.features,
    )?;
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
    spawn_pi(state, command, mcp, delivery, None)
}

/// A resumed child: the same spawn, with `--session <peer>` spliced onto the
/// launch argv and nothing else changed. Pi loads the conversation from its
/// own session file by the id the dead generation persisted — the
/// `sessionId` `get_state` reports and this client already reads off the
/// wire. Nothing from our journal is re-sent; the provider's session file is
/// the conversation.
pub(super) fn spawn_process_resuming(
    state: &Arc<ServerState>,
    command: PtyCommand,
    peer_session_id: String,
    mcp: Option<crate::mcp_broker::McpLaunchConfig>,
) -> Result<SpawnedSession, WireError> {
    spawn_pi(
        state,
        command,
        mcp,
        ProfileDelivery::none(),
        Some(peer_session_id),
    )
}

/// The child both roads share: same process, same carrier, same handshake.
/// `resume_session` is `Some(peer)` only on the resume road, where the pair
/// is spliced onto the argv; every other line is identical, so a resumed
/// child cannot drift from a fresh one.
fn spawn_pi(
    state: &Arc<ServerState>,
    command: PtyCommand,
    mcp: Option<crate::mcp_broker::McpLaunchConfig>,
    delivery: ProfileDelivery,
    resume_session: Option<String>,
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
    let args = match resume_session.as_deref() {
        Some(peer_session_id) => spawn_resume_args(
            &command,
            &extension_path,
            bridge_path.as_deref(),
            peer_session_id,
        )?,
        None => spawn_args(&command, &extension_path, bridge_path.as_deref())?,
    };
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
        // This agent's own fresh job; why no shared job is ever an
        // assignment target is stated once, at open_pty_session.
        if let Err(error) = process_job.assign(handle) {
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
    // The list request goes out immediately after the handshake (Paseo asks
    // once, `pi/agent.ts:1658-1664`): registered and written, never awaited
    // on this thread — the reply was measured to take tens of seconds, and
    // neither the session's start nor a prompt may sit on it. The reader
    // starts the waiter that turns the reply into the menu.
    let commands_reply = commands::begin_get_commands(&control);
    // Paseo's `tryHandleOutOfBand` dispatch (`pi/agent.ts:1667-1691`): pi is
    // the only family whose side-effect commands exist today, so pi is the
    // only spawn that builds the seam a send consults. The compact slot it
    // owns is shared with the reader, which observes pi's own compaction
    // frames — Paseo keeps both halves in one agent (`:2344-2356`).
    let handler = out_of_band::PiOutOfBandCommands::new(Arc::clone(&control));
    let compact_guard = handler.compact_guard();
    let out_of_band: Option<Arc<dyn OutOfBandCommands>> = Some(Arc::new(handler));
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
    .with_extension_path(extension_path.clone())
    .with_commands_reply(commands_reply)
    .with_compact_guard(compact_guard);
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
        out_of_band,
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
        _raw_text: &str,
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
        // The send rides the same critical section as the removal: a waiter
        // timing out meets either its own entry (it owns the outcome) or the
        // answer already on its channel (the reader owns it) — never neither.
        // The channel is unbounded so the send cannot block; the clone is the
        // only work held under the lock.
        self.pending
            .lock()
            .ok()
            .map(|mut pending| {
                pending
                    .remove(id)
                    .is_some_and(|sender| sender.send(Ok(value.clone())).is_ok())
            })
            .unwrap_or(false)
    }

    /// Give up one registration without answering it. `true` when the entry
    /// was still held — the waiter owns the outcome. `false` only when the
    /// reader already claimed it *and* put the answer on the waiter's
    /// channel, which `deliver` does under the same lock — so a timed-out
    /// waiter and its late reply can never both act on one answer, and one
    /// of them always does. A poisoned lock also answers `true`: `deliver`
    /// fails on the same lock, so the waiter publishing the seeds is the
    /// exactly-one.
    fn abandon(&self, id: &str) -> bool {
        self.pending
            .lock()
            .map(|mut pending| pending.remove(id).is_some())
            .unwrap_or(true)
    }

    /// Wake every waiter still registered with the reason the control channel
    /// ended (A2-02).
    ///
    /// A response can no longer arrive for any of them — the child's output is
    /// what delivers responses, and it is over — so a waiter left registered
    /// would sit out its whole timeout for an answer that cannot come. The map
    /// is drained and each sender answered under the same lock, so a timeout
    /// landing mid-wake meets either its entry or the channel's end.
    fn fail_pending(&self, message: &str) {
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        // The sends ride the drain's critical section, for the same reason as
        // `deliver`'s: a timeout landing here meets either its entry or the
        // channel's end, never neither.
        for (_, sender) in pending.drain() {
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
        // Paseo refuses to steer a slash input (`pi/agent.ts:1417-1419`:
        // "Pi rejects steer RPCs that are extension commands"), so it keeps
        // the interrupt-and-replace fallback where the text can run
        // directly. Refused before a frame is written, which is what makes
        // this `Ok(false)` and not a transport error.
        if commands::parse_slash_invocation(text).is_some() {
            return Ok(false);
        }
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
    commands_reply: Option<commands::PiCommandsReply>,
    compact: Arc<out_of_band::CompactGuard>,
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
            commands_reply: None,
            compact: Arc::new(out_of_band::CompactGuard::default()),
            extension_path: PathBuf::new(),
        }
    }

    fn with_extension_path(mut self, extension_path: PathBuf) -> Self {
        self.extension_path = extension_path;
        self
    }

    /// The list request the spawn wrote, handed over so the first feed can
    /// start its waiter against the live runtime.
    fn with_commands_reply(mut self, reply: commands::PiCommandsReply) -> Self {
        self.commands_reply = Some(reply);
        self
    }

    /// The compact slot shared with the out-of-band handler, so pi's own
    /// compaction frames end the run they belong to (Paseo
    /// `pi/agent.ts:2344-2356`).
    fn with_compact_guard(mut self, compact: Arc<out_of_band::CompactGuard>) -> Self {
        self.compact = compact;
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
        if value.get("type").and_then(Value::as_str) == Some("response") {
            let claimed = self.control.deliver(&value);
            // Paseo finds no pending entry for such a response and returns
            // without effect (`jsonl-rpc-process.ts:157-163, 283-292`); ours
            // must do the same, and harder: this row would be replay's copy
            // of the list, so a `get_commands` reply that answered nothing —
            // an expired id, a foreign one — reaches neither the transcript
            // nor the journal (review A5-2 #1). A claimed reply is two
            // things at once: the waiter's answer (delivered above) and a
            // row whose derivation is the published list, carrying the row's
            // sequence like every other row.
            if value.get("command").and_then(Value::as_str) == Some("get_commands") && !claimed {
                return Ok(());
            }
            let event_seq = runtime.journal_agent_envelope(&value);
            for event in crate::pi_view::events_from_line(&value) {
                self.publish(runtime, event, event_seq);
            }
            return Ok(());
        }
        // A late end for a run the transcript already completed synthetically
        // leaves neither a row nor an event, so replay re-derives exactly what
        // live showed instead of a second completion marker.
        if self.compact.observe(&value) {
            return Ok(());
        }
        let event_seq = runtime.journal_agent_envelope(&value);
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
        // Every event derived from one row carries that row's journal seq —
        // the contract claude, acp and codex keep — so an attach's replay
        // seam can match a backlog copy against the copy replay derives from
        // the same row. A `None` here would make the finish's context reading
        // survive the seam beside its replayed twin and deliver twice.
        for event in crate::pi_view::events_from_line(&value) {
            self.publish(runtime, event, event_seq);
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
            if !matches!(error, PermissionResponseError::AlreadyRecorded) {
                // The repeat-refusal's one plain notice is already up; only
                // the extension response goes back for that case.
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!("Could not queue Pi permission request: {error}"),
                    },
                    event_seq,
                );
            }
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
        is_chooser: None,
        kind: None,
        questions: None,
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
            // The first feed is the first moment this reader holds a runtime,
            // and `reader_loop` makes it with an empty read before any child
            // output arrives — so the list waiter starts here, detached from
            // every send path that must not wait for pi's slow reply.
            if let Some(reply) = self.commands_reply.take() {
                commands::spawn_commands_waiter(reply, Arc::clone(runtime));
            }
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
#[path = "pi_client_tests.rs"]
mod tests;
