//! Provider discovery for the command-line agents available to Devboule.
//!
//! The six-agent alias table is adapted from herdr, licensed under the Apache
//! License, Version 2.0, commit `3150bd9`, specifically
//! `herdr-src/src/detect/mod.rs:149-218`.
//! The executable-file check is likewise adapted from herdr under that
//! license and commit, from `herdr-src/src/integration/registry.rs:161-179`.
//! The launch resolver is Devboule code: it follows PATHEXT but retains only
//! extensions that `std::process::Command` can launch directly. On Windows it
//! then unwraps an npm `cmd-shim` `.cmd`/`.bat` to `node` plus the package
//! script, so CreateProcess does not go through `cmd.exe`. That unwrap is the
//! inverse of herdr's `normalized_process_name` /
//! `agent_name_from_known_package_path` (`herdr-src/src/detect/mod.rs:359-650`,
//! commit `3150bd9`), which identify a running `node.exe` agent from argv.
//! The provider catalog shape, ACP launch metadata, and explicit unknown
//! authentication state are also Devboule code.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::{Command, Output};

#[cfg(windows)]
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";
pub(crate) const MAX_EXTERNAL_VERSION_CHARS: usize = 64;

/// Keep externally supplied version labels safe and bounded before they enter
/// the wire contract or an in-process cache. Control and bidi characters are
/// stripped rather than replaced because version labels have no legitimate
/// control characters, and replacement could change the apparent version.
pub(crate) fn cap_external_version(value: &str) -> Option<String> {
    let capped = strip_control_and_bidi(value)
        .chars()
        .take(MAX_EXTERNAL_VERSION_CHARS)
        .collect::<String>();
    (!capped.is_empty()).then_some(capped)
}

/// The characters an externally supplied string may keep: no control
/// characters, no invisible formatting. [`cap_external_version`] applies this
/// to version labels; the peer-roster boundary (`mcp_peer_agents`) applies
/// the same set to a far daemon's roster text. The invisible set is the
/// shared table, so the two paths agree by construction rather than by
/// coincidence.
pub(crate) fn strip_control_and_bidi(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            !character.is_control() && !crate::text_safety::is_invisible_format(*character)
        })
        .collect()
}

/// A known CLI name and its aliases, with the ACP invocation when supported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownAgent {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub acp_args: Option<&'static [&'static str]>,
    /// Native stream-json argv, used only by the Claude adapter. Other
    /// agents stay on ACP; this field is None for them.
    pub stream_json_args: Option<&'static [&'static str]>,
    /// Native Pi RPC argv, used only by the Pi adapter.
    pub rpc_args: Option<&'static [&'static str]>,
    /// Native Codex app-server argv, used only by the Codex adapter.
    pub app_server_args: Option<&'static [&'static str]>,
    pub npm_package: Option<&'static str>,
}

/// Agents currently relevant to the first provider catalog slice.
///
/// Keep one row per agent so adding a known CLI does not require changing the
/// discovery algorithm.
/// Measured CLI 2.1.260 stream-json host-permission launch. Empty
/// `--setting-sources` is a literal empty argument, not an omitted flag.
pub const CLAUDE_STREAM_JSON_ARGS: &[&str] = &[
    "-p",
    "--output-format",
    "stream-json",
    "--input-format",
    "stream-json",
    "--verbose",
    "--include-partial-messages",
    "--forward-subagent-text",
    "--strict-mcp-config",
    "--setting-sources",
    "",
    "--permission-prompts",
    "host",
    "--permission-prompt-tool",
    "stdio",
];

pub const KNOWN_AGENTS: &[KnownAgent] = &[
    KnownAgent {
        id: "claude",
        aliases: &["claude", "claude-code"],
        acp_args: None,
        stream_json_args: Some(CLAUDE_STREAM_JSON_ARGS),
        rpc_args: None,
        app_server_args: None,
        npm_package: Some("@anthropic-ai/claude-code"),
    },
    KnownAgent {
        id: "codex",
        aliases: &["codex"],
        acp_args: None,
        stream_json_args: None,
        rpc_args: None,
        app_server_args: Some(&["app-server"]),
        npm_package: Some("@openai/codex"),
    },
    KnownAgent {
        id: "grok",
        aliases: &["grok", "grok-build"],
        acp_args: Some(&["agent", "stdio"]),
        stream_json_args: None,
        rpc_args: None,
        app_server_args: None,
        npm_package: None,
    },
    KnownAgent {
        id: "pi",
        aliases: &["pi"],
        acp_args: None,
        stream_json_args: None,
        // The Pi adapter validates this mode and adds its permission
        // extension after the caller's arguments.
        rpc_args: Some(&["--mode", "rpc"]),
        app_server_args: None,
        npm_package: None,
    },
    KnownAgent {
        id: "qwen",
        aliases: &["qwen", "qwen-code", "qwen code"],
        // Measured 2026-09-04: `--acp` answers `initialize` and
        // `--experimental-acp` still works but prints "deprecated and will be
        // removed in a future release. Please use --acp instead."
        acp_args: Some(&["--acp"]),
        stream_json_args: None,
        rpc_args: None,
        app_server_args: None,
        npm_package: Some("@qwen-code/qwen-code"),
    },
    KnownAgent {
        id: "gemini",
        aliases: &["gemini"],
        acp_args: Some(&["--acp"]),
        stream_json_args: None,
        rpc_args: None,
        app_server_args: None,
        npm_package: Some("@google/gemini-cli"),
    },
];

// Only the `server` provider-update path needs an id -> package lookup; the
// `npm_package` field itself stays public on `KnownAgent` for other readers.
#[cfg(feature = "server")]
pub(crate) fn known_npm_package(id: &str) -> Option<&'static str> {
    KNOWN_AGENTS
        .iter()
        .find(|agent| agent.id == id)
        .and_then(|agent| agent.npm_package)
}

// Test-only provider: the integration stub binary. It resolves only when its
// build directory is on PATH, so end-user machines never list it; release
// builds drop the row entirely so a shipped daemon cannot discover it. The
// row exists so provider health measured against the stub (spawn + handshake
// outcomes) is visible through ProvidersList in integration tests, mirroring
// how real providers are surfaced.
#[cfg(debug_assertions)]
const TEST_ONLY_AGENTS: &[KnownAgent] = &[
    KnownAgent {
        id: "devboule-acp-stub",
        aliases: &["devboule-acp-stub"],
        acp_args: None,
        stream_json_args: None,
        rpc_args: None,
        app_server_args: None,
        npm_package: None,
    },
    // A provider id whose binary cannot exist anywhere (`S5` e2e): it is what
    // makes `provider not installed` reachable without depending on what the
    // machine running the tests happens to have installed. Like the stub it is
    // `#[cfg(debug_assertions)]` only, and its preset cell lives in
    // [`test_only_cell`], so a release daemon knows neither.
    KnownAgent {
        id: "devboule-absent-probe",
        aliases: &["devboule-absent-probe"],
        acp_args: None,
        stream_json_args: None,
        rpc_args: None,
        app_server_args: None,
        npm_package: None,
    },
];
#[cfg(not(debug_assertions))]
const TEST_ONLY_AGENTS: &[KnownAgent] = &[];

/// Every tool the daemon's MCP broker can serve, in `tools/list` order.
///
/// One source of truth for the broker's `tools/list` body and for the
/// `ProviderInfo.tools` the Settings panel renders, so the panel and the wire
/// cannot disagree about a tool's name or its description.
pub const MCP_BROKER_TOOLS: &[(&str, &str)] = &[
    (
        MCP_ROSTER_TOOL,
        "Lists live Devboule agent sessions known by the daemon, with their display name, the session that created them, their lifecycle state and their creation depth.",
    ),
    (
        MCP_LIST_DEVICES_TOOL,
        "Lists the devices this machine is paired with: each device's id (the name to use when referring to it), its display name, its role, and whether it currently has a live connection to this machine. Answers locally, without contacting the other devices.",
    ),
    (
        MCP_LIST_PEER_AGENTS_TOOL,
        "Lists the agents running right now on one paired device, named by deviceId from devboule_list_devices. Each agent is identified by the pair of device id and session id, and carries its name, provider, model, state and creation depth. This is what is live on that device at the moment of the call, not a stored list, and the device is dialled once per call; a cold connection can take several seconds.",
    ),
    (
        MCP_LIST_PROFILES_TOOL,
        "Lists the agent profiles the human enabled for agents, in the human's own order, with the note that says when to use each one. Call this before devboule_create_agent. Each profile's unattended field is a prediction: \"yes\" means a session created from it approves its own permission prompts, \"no\" means it asks the human, \"unknown\" means Devboule cannot promise either way - the child may stop on its first permission card.",
    ),
    (
        MCP_SEND_MESSAGE_TOOL,
        "Sends a message to one live Devboule agent session.",
    ),
    (
        MCP_CREATE_AGENT_TOOL,
        "Creates a new Devboule agent session from a profile the human enabled for agents, and sends it an initial prompt. The human is asked to authorize the first creation from this session; the result is the new session's id, its A2A task and context, and its display name. With notifyOnFinish false the child is also exempt from the idle (quiet) notice.",
    ),
    (
        MCP_SET_AGENT_PROFILE_TOOL,
        "Moves one of your own live child sessions onto a profile the human enabled for agents: the child is asked to switch to the profile's mode, then to the profile's model and thinking option, and the profile is recorded on the child. The human is never asked, and the child is never restarted; a provider that refuses the switch refuses the move. Moving onto a profile that runs unattended is permanent - the child's row keeps the marker even if it is moved back.",
    ),
    (
        MCP_ANSWER_PERMISSION_TOOL,
        "Answers one pending permission card of one of your own live children, when the human has turned permission delegation on. The card reaches you as an agent_permission_request notice naming its cardId. outcome is allow_once or deny - never anything durable, and never a card that is not your child's. The human still sees the card either way.",
    ),
    (
        MCP_ACTIVITY_TOOL,
        "Reads what one live agent session of your own owner has been doing: its current activity (working, idle, blocked or unknown), how long since it last published, and its recent event kinds with timestamps. Metadata only, never transcript text. Name the session by id or display name; limit caps the recent lines (default 10, max 50; 0 returns the state with no recent lines).",
    ),
    (
        MCP_STOP_AGENT_TOOL,
        "Stops one of your own live child sessions: its process tree is killed and the child stops running, while its session row and transcript stay in history. Use this for a child that is stuck or that you no longer need running. Name the child by id or display name; you can only stop a session you created yourself.",
    ),
    (
        MCP_CLOSE_AGENT_TOOL,
        "Ends one of your own live child sessions: the live session goes away and its transcript stays in history. Use this to finish with a child you created and no longer need. Name the child by id or display name; you can only close a session you created yourself.",
    ),
    (
        MCP_CANCEL_AGENT_TOOL,
        "Interrupts the current turn of one of your own live child sessions and keeps the child: the child stops what it is doing now, any permission card it had parked is resolved as interrupted, and it stays alive for your next message. This is the soft verb between doing nothing and devboule_stop_agent, which kills the process. Name the child by id or display name; you can only cancel a session you created yourself. The cancel reaches the child's session, not the turn: a turn that starts before the provider processes it may be cancelled too. Replies success: true when the turn that was running when you called is no longer running; success: false when no turn was running, or when that turn did not stop within the two-second wait - the text says which.",
    ),
    (
        MCP_LIST_PENDING_PERMISSIONS_TOOL,
        "Lists the permission cards your own live children are parked on right now, whatever the human's delegation switch says: each card's agentId, cardId, title, kind and a short excerpt of what the child asked. Listing is a read of your own children only; answering a card still requires the human's delegation switch and goes through devboule_answer_permission. truncated: true means more cards are pending than were listed: devboule_get_agent_status counts them per child, and answering some of the listed cards frees room for later calls while the human's delegation switch is on. A child with no cards adds no entry, and an empty list means nothing is parked.",
    ),
    (
        MCP_GET_AGENT_STATUS_TOOL,
        "Reads one of your own children as a snapshot: its state (a parked card shows as input_required, a closed child reads as closed), provider, model, mode, profile, who created it, its depth, how long since it last published, and how many permission cards it is parked on - their ids, titles and excerpts come only from devboule_list_pending_permissions. Name a live child by id or display name and a closed child by its id; you can only read a session you created yourself. Anything else - a sibling, a stranger's session, an invented id - reads as not found.",
    ),
    (
        MCP_NEIGHBORHOOD_TOOL,
        "Walks the project's code-knowledge graph from one node and answers the nodes reachable within a number of edges, each with its shortest distance from the node you named. The graph belongs to the calling session's own workspace: the indexer builds it from that folder's files, and a node id is a repository-relative path (a file) or that path with a '#start-end-index' suffix (a symbol inside it). depth is 1 to 4 edges (default 1); kind filters on the graph's two edge kinds, IMPORT and CONTAIN. Topology only: no source text, no symbol bodies, no semantic search. A node the graph does not contain answers with an empty list, exactly like a node with no edges. Fails when the session has no workspace, and when that workspace has no graph yet - never by reading another project's graph.",
    ),
    (
        MCP_IMPORTS_TOOL,
        "Answers which files one file imports, from the project's code-knowledge graph in the calling session's own workspace. file is named by its repository-relative path as the graph spells it; a symbol id names a symbol rather than a file and answers with an empty list, as does a path the graph does not know. Import edges only: the file-to-file dependencies the indexer resolved inside the indexed set, never a guess at the filesystem. For the reverse direction use devboule_project_importers; this tool answers no source text and no semantic search.",
    ),
    (
        MCP_IMPORTERS_TOOL,
        "Answers which files import one file - the reverse of devboule_project_imports, read from the project's code-knowledge graph in the calling session's own workspace. file is named by its repository-relative path as the graph spells it. Import edges only, never call edges: 'who calls this function' is a question this graph cannot answer, and this tool does not answer it with an empty list that would look like 'nobody does'.",
    ),
    (
        MCP_ORACLE_SEARCH_TOOL,
        "Answers questions about the code of the calling session's own workspace by meaning, not by keyword: the Oracle index built for that folder is searched and the closest chunks come back as citations. query is the question in natural language; limit is how many chunks to return (1 to 10, default 10). Each result carries a repository-relative path, a line range, a narrower focus span when one was scored, the chunk text, a score that is a rank fusion (RRF) rather than a cosine similarity, and whether the chunk was found densely, lexically, or both. The folder searched is the calling session's own workspace, taken from the session's row and never from an argument; a session with no workspace is refused. This tool needs the Devboule desktop app running: the engine, the index and the local models live in the app, so with the app closed the call fails with a sentence that says exactly that. The project-graph tools (devboule_project_neighborhood, devboule_project_imports, devboule_project_importers) do not need the app. Fail-closed: no index, no model, no vectors, or a model still loading each answers with its own reason and the action to take - never an empty result list standing in for a missing fact, and never an answer from another project's index.",
    ),
    (
        MCP_LIST_WORKSPACES_TOOL,
        "Lists the workspaces of the calling session's own project: each workspace's id, name, checkout path, kind and branch. Only the caller's project is ever listed, and the project comes from the session's row, never from an argument.",
    ),
    (
        MCP_CREATE_WORKSPACE_TOOL,
        "Creates a workspace inside the calling session's own project, as the project folder itself or a new git worktree beside it, and answers the new workspace record. The human is asked to approve workspace writes from this session the first time. branch names the worktree branch and is worktree-only; name sets the workspace title. No path is accepted: the checkout is the project folder or a sibling worktree of it, never an agent-named directory.",
    ),
    (
        MCP_LIST_TERMINALS_TOOL,
        "Lists the terminal sessions in the calling session's own workspace: each one's id, title, working directory, the session that created it, and whether its process is still running. Terminals only — never agent sessions — and only those of the calling session's own user inside the calling session's own workspace; a terminal of another workspace or another owner is not in the list. The workspace comes from the calling session's row, never from an argument, so a session started outside any workspace is refused rather than shown every workspace-less terminal of its user.",
    ),
    (
        MCP_CAPTURE_TERMINAL_TOOL,
        "Reads the visible screen of one terminal as plain text: at most the number of lines you ask for, counted from the bottom of the grid (default 40, at most 200), with escape sequences stripped. Never the scrollback — the terminal's visible grid only, since the journal is the durable transcript. The terminal must be one of the calling session's own user inside the calling session's own workspace, and it must still be running; any other id — an agent session, another owner's terminal, another workspace's terminal, or one that has exited — answers 'No session with that id.', which never says which of them the id named.",
    ),
];

/// The read-only roster tool, and the one name a tool policy can never
/// disable: an agent that cannot list its siblings cannot be steered at all,
/// and the tool reads only its own bearer's roster.
pub const MCP_ROSTER_TOOL: &str = "devboule_list_agents";
/// The read-only paired-device discovery tool: the answer that lets an agent
/// name a device at all. Answered from this daemon's own `peers` rows — never
/// dialled — and scoped to the calling session's own user, with the key, the
/// address and the binding fields withheld: an agent needs to name a device,
/// not to audit it.
pub const MCP_LIST_DEVICES_TOOL: &str = "devboule_list_devices";
/// The one-dial peer roster tool: name one paired device, ask it for the
/// agents it is running now. The dial is bounded by the outbound transport's
/// own deadlines, and the answer is a liveness snapshot — never a stored
/// object, and never a fan-out: one call names one device and makes one dial.
pub const MCP_LIST_PEER_AGENTS_TOOL: &str = "devboule_list_peer_agents";
pub const MCP_SEND_MESSAGE_TOOL: &str = "devboule_send_message";
/// The creation tool (`S5`).
///
/// Served to every MCP-capable provider, like the two above, but **not**
/// deliberately set in the always-on list that protects the roster: §2 puts it
/// "subject to the provider tool policy like `devboule_send_message`", so a
/// stored policy may turn agent creation off for a provider, and turning it off
/// is the safe direction.
pub const MCP_CREATE_AGENT_TOOL: &str = "devboule_create_agent";
/// The move tool (slice 5b §2, Pass A): a creator moves its own live child
/// onto a named ticked profile.
///
/// Served to every MCP-capable provider, subject to the provider tool policy
/// like `devboule_create_agent`. The name carries "profile" on purpose — it is
/// the one write-shaped companion to the create tool on this surface — but the
/// profile store is only ever **read** through the same resolver the create
/// tool uses; nothing a caller sends reaches the store's own RPCs, and the
/// authority for the move is the `created_by` link, never the name.
pub const MCP_SET_AGENT_PROFILE_TOOL: &str = "devboule_set_agent_profile";
/// The delegated permission answer (`slice 5b`).
///
/// Served to every MCP-capable provider, subject to the provider tool policy
/// like `devboule_send_message` and `devboule_create_agent`: a stored policy
/// may take the answer tool away from a provider, and taking it away is the
/// safe direction. The name carries neither `delegat` nor `grant` on purpose
/// — the broker table must not reach the delegation switch, which has no
/// tool at all.
pub const MCP_ANSWER_PERMISSION_TOOL: &str = "devboule_answer_permission";
/// The read-only activity tool: one agent's derived headline plus its bounded
/// recent kinds. A read like the roster, subject to the provider tool policy
/// like `devboule_send_message` — a stored policy may take supervision away,
/// and taking it away is the safe direction. Never always-on: the roster and
/// the profile list stay the two tools a policy cannot remove, because without
/// them an agent cannot work at all; without this one it only cannot watch.
pub const MCP_ACTIVITY_TOOL: &str = "devboule_agent_activity";
/// The stop tool: a creator stops one of its own live children — the
/// process tree dies, the row and its transcript stay (`session.rs::stop`).
///
/// Served to every MCP-capable provider, subject to the provider tool policy
/// like `devboule_send_message`: a stored policy may take supervision away,
/// and taking it away is the safe direction. The act is destructive, so the
/// peer door judges it as the wire's `SessionStop`, which rides the
/// administrative capability — a device the owner granted the whole surface
/// reaches it, one without is refused.
pub const MCP_STOP_AGENT_TOOL: &str = "devboule_stop_agent";
/// The close tool: a creator ends one of its own live children — the live
/// session goes away, the transcript stays in history. Same policy subject
/// and same construction as [`MCP_STOP_AGENT_TOOL`], judged as
/// the wire's `SessionClose`: the administrative capability is what opens it.
pub const MCP_CLOSE_AGENT_TOOL: &str = "devboule_close_agent";
/// The cancel tool: a creator interrupts the current turn of one of its own
/// live children and keeps the child — the soft verb between doing nothing
/// and [`MCP_STOP_AGENT_TOOL`].
///
/// Served to every MCP-capable provider, subject to the provider tool policy
/// like `devboule_send_message`: a stored policy may take supervision away,
/// and taking it away is the safe direction. The child's parked permission
/// cards resolve as interrupted, and the child stays alive for the next
/// message. The peer door judges it as the wire's `SessionInterrupt`, which
/// rides the administrative capability — the same gate the wire's interrupt,
/// stop and close already meet.
pub const MCP_CANCEL_AGENT_TOOL: &str = "devboule_cancel_agent";
/// The pending-permission list: the cards the caller's own live children are
/// parked on right now — a pull beside the push envelope, so a coordinator
/// can recover a `cardId` it was never surfaced.
///
/// Served to every MCP-capable provider, subject to the provider tool policy
/// like `devboule_send_message`. A read of one's own children: it lists while
/// the human's delegation switch is off, because the switch gates answering,
/// never seeing — and answering still runs its full checks through
/// [`MCP_ANSWER_PERMISSION_TOOL`]. The peer door judges it as the wire's
/// `SessionPermissionRespond`, never weaker than the answer tool that
/// consumes its `cardId`.
pub const MCP_LIST_PENDING_PERMISSIONS_TOOL: &str = "devboule_list_pending_permissions";
/// The status tool: one of the caller's own children as a snapshot — state,
/// provider, model, mode, profile, creator, depth, idle age and the cards it
/// is parked on. A closed child answers from its stored row with no pending
/// permissions; anything that is not the caller's own child reads as not
/// found.
///
/// Served to every MCP-capable provider, subject to the provider tool policy
/// like `devboule_send_message`. A read like the roster — the peer door
/// judges it as the wire's `SessionsList`, the act `view` names.
pub const MCP_GET_AGENT_STATUS_TOOL: &str = "devboule_get_agent_status";
/// The read-only profile-list tool (`create-from-profile`).
///
/// Served to every MCP-capable provider, and **always on**, like the roster
/// tool: the creation tool names a profile and nothing else, so a caller that
/// cannot list the profiles cannot create anything at all. That is the same
/// reasoning the always-on roster tool gets — the tool that is the only way to
/// say who to talk to is the tool a stored policy must not be able to take
/// away — and it is why [`crate::tool_policy::is_tool_enabled`] answers for
/// this name before it reads a policy.
pub const MCP_LIST_PROFILES_TOOL: &str = "devboule_list_profiles";
/// The three project-graph tools: the code-knowledge graph the indexer writes
/// for the calling session's own workspace, read-only.
///
/// The graph is derived from the files of a workspace, and a local agent
/// already reads those files (its cwd is that workspace), so the tools hand it
/// no reach it did not have; a paired device's agent does not read this
/// machine's files, so the same read discloses the project's structure over the
/// wire — which is why the door judges them as the wire's workspace inventory
/// read, and why that read must be the administrative capability and not
/// `view`: the graph discloses more per workspace than the inventory does, not
/// less - see `peer_policy::mcp_tool_wire`. The workspace comes from the
/// caller's session row, never from an argument.
pub const MCP_NEIGHBORHOOD_TOOL: &str = "devboule_project_neighborhood";
/// Which files one file imports, from the same workspace graph.
pub const MCP_IMPORTS_TOOL: &str = "devboule_project_imports";
/// Which files import one file: the reverse direction, same graph.
pub const MCP_IMPORTERS_TOOL: &str = "devboule_project_importers";
/// The semantic search tool: the Oracle index of the calling session's own
/// workspace, read by meaning, served by the desktop app's engine.
///
/// Same door verdict as the three graph tools it complements — and with the
/// stronger premise: it ships source snippets (content), not topology, so it
/// rides the same administrative capability rather than a weaker one. The
/// workspace comes from the caller's session row, never from an argument.
/// Unlike the graph tools this one needs the app open: the engine, the index
/// and the local models live in the app, so the description says so and the
/// tool's refusal when the app is closed says so too.
pub const MCP_ORACLE_SEARCH_TOOL: &str = "devboule_oracle_search";
/// The workspace inventory read: the workspaces of the calling session's own
/// project, scoped through its session row.
///
/// A read like the roster, subject to the provider tool policy like
/// `devboule_send_message` — a stored policy may take the inventory away,
/// and taking it away is the safe direction. The sibling checkouts it names
/// are the caller's own project, so the door judges it as the wire's
/// workspace inventory read, under the administrative capability.
pub const MCP_LIST_WORKSPACES_TOOL: &str = "devboule_list_workspaces";
/// The workspace write: a new checkout inside the calling session's own
/// project, behind the first-use human card.
///
/// Served to every MCP-capable provider, subject to the provider tool policy
/// like `devboule_create_agent`, and denied to the design preset outright:
/// a design child commissions no checkouts. The door judges it as the wire's
/// `WorkspaceCreate`, under the administrative capability.
pub const MCP_CREATE_WORKSPACE_TOOL: &str = "devboule_create_workspace";
/// The terminal roster tool: the terminals of the caller's own owner inside
/// the caller's own workspace, live and exited alike — a read like the
/// roster, subject to the provider tool policy like `devboule_send_message`:
/// a stored policy may take it away, and taking it away is the safe
/// direction. Never always-on. It hands out metadata only (id, title,
/// directory, creator, running or not), never a screen.
pub const MCP_LIST_TERMINALS_TOOL: &str = "devboule_list_terminals";
/// The terminal screen read: the visible grid of one in-scope running
/// terminal, as plain text with no escape sequence and no scrollback.
///
/// Same policy subject as [`MCP_LIST_TERMINALS_TOOL`], and the stronger
/// premise: a screen is content — it can hold a prompt, a token or a
/// password — so the peer door requires the administrative capability for
/// it, the capability that opens the project graph, and not the weaker
/// metadata reads. Scope is the caller's owner and workspace from the rows,
/// plus `kind == Terminal`: an agent session's id answers "not found",
/// because a screen behind that id belongs to a provider's stdin, not to a
/// terminal.
pub const MCP_CAPTURE_TERMINAL_TOOL: &str = "devboule_capture_terminal";

/// The `tools/list` input schema of [`MCP_CREATE_AGENT_TOOL`] (`S5` §2).
///
/// Closed on purpose. `additionalProperties: false` is the schema's half of a
/// two-part rule: the broker refuses an unknown parameter by name, and the
/// broker's own list of known parameters is read *out of this document* — so a
/// parameter can never be described here and unchecked there.
///
/// There is deliberately no `provider`, `model`, `mode`, `settings`, `features`
/// or `preset`: a profile the human wrote and ticked is the only way to say what
/// to run, and what an agent cannot express is what no check can get wrong.
/// `labels` is a free map of strings with the `devboule.` prefix reserved for
/// the daemon's own facts. `notifyOnFinish` defaults to true.
#[cfg(feature = "server")]
pub(crate) fn agent_create_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "profile": {
                "type": "string",
                "description": "Name of a profile the human enabled for agents; see devboule_list_profiles."
            },
            "title": {
                "type": "string",
                "description": "The child's display name, 1 to 60 characters."
            },
            "labels": {
                "type": "object",
                "description": "Optional labels for the child: string to string. The devboule. prefix is reserved.",
                "additionalProperties": {"type": "string"}
            },
            "workspaceId": {
                "type": "string",
                "description": "A workspace of the same owner; defaults to the caller's."
            },
            "cwd": {
                "type": "string",
                "description": "A directory inside that workspace, relative to it."
            },
            "initialPrompt": {
                "type": "string",
                "description": "The child's first prompt, at most 32 KiB."
            },
            "notifyOnFinish": {
                "type": "boolean",
                "description": "Whether this session is told when the child finishes. Default true."
            }
        },
        "required": ["profile", "title", "initialPrompt"],
        "additionalProperties": false
    })
}

/// The `tools/list` input schema of [`MCP_SET_AGENT_PROFILE_TOOL`] (slice 5b
/// §2).
///
/// Closed on purpose, like [`agent_create_input_schema`], and rhyming with the
/// landed create schema: the profile argument is the profile's **name**, the
/// same way `devboule_create_agent` names one. There is deliberately no
/// caller, owner, or creator parameter — identity is imposed by the broker
/// from the bearer's registration (`registration.session_id`), never stated by
/// the caller — and no provider, model, mode or feature parameter: a profile
/// the human wrote and ticked is the only way to say what to run, read at the
/// moment of the call.
#[cfg(feature = "server")]
pub(crate) fn agent_set_profile_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "session": {
                "type": "string",
                "description": "The id or display name of one of your own live child sessions."
            },
            "profile": {
                "type": "string",
                "description": "Name of a profile the human enabled for agents; see devboule_list_profiles."
            }
        },
        "required": ["session", "profile"],
        "additionalProperties": false
    })
}

/// The `tools/list` input schema of [`MCP_ACTIVITY_TOOL`].
///
/// Closed on purpose, like the create schema: the child is named by id or
/// display name (the broker resolves both, like `devboule_send_message`'s
/// `to_agent`), and `limit` is optional. Identity is never a parameter — it
/// is imposed from the bearer's registration.
#[cfg(feature = "server")]
pub(crate) fn agent_activity_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "session": {
                "type": "string",
                "description": "The id or display name of one live agent session of your own owner."
            },
            "limit": {
                "type": "integer",
                "minimum": 0,
                "maximum": 50,
                "description": "How many recent event lines to return. Default 10, max 50."
            }
        },
        "required": ["session"],
        "additionalProperties": false
    })
}

/// The `tools/list` input schema shared by [`MCP_STOP_AGENT_TOOL`] and
/// [`MCP_CLOSE_AGENT_TOOL`].
///
/// One schema, not two: the verbs differ in what they do, not in what they
/// accept, and sharing keeps that fact load-bearing. Closed on purpose, like
/// the create schema; there is deliberately no caller or creator parameter —
/// identity is imposed by the broker from the bearer's registration.
#[cfg(feature = "server")]
pub(crate) fn agent_end_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "session": {
                "type": "string",
                "description": "The id or display name of one of your own live child sessions."
            }
        },
        "required": ["session"],
        "additionalProperties": false
    })
}

/// The `tools/list` input schema shared by [`MCP_CANCEL_AGENT_TOOL`] and
/// [`MCP_GET_AGENT_STATUS_TOOL`].
///
/// One schema, not two: both name one of the caller's own children, and
/// sharing keeps that fact load-bearing. Closed on purpose, like the create
/// schema; there is deliberately no caller or creator parameter — identity is
/// imposed by the broker from the bearer's registration. The value resolves
/// by id or display name, the same two doors the other child tools take.
#[cfg(feature = "server")]
pub(crate) fn agent_id_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "agentId": {
                "type": "string",
                "description": "The id or display name of one of your own live child sessions."
            }
        },
        "required": ["agentId"],
        "additionalProperties": false
    })
}

/// The `tools/list` input schema of [`MCP_LIST_PEER_AGENTS_TOOL`].
///
/// Closed on purpose, like the create schema: the device is named by the id
/// `devboule_list_devices` answered, and nothing else is a parameter. There
/// is deliberately no user, owner or scope argument — whose roster the
/// responder answers with is the responder's own fact (the user that
/// approved the pairing), never something a caller could widen.
#[cfg(feature = "server")]
pub(crate) fn peer_agents_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "deviceId": {
                "type": "string",
                "description": "The id of one paired device, from devboule_list_devices."
            }
        },
        "required": ["deviceId"],
        "additionalProperties": false
    })
}

/// The `tools/list` input schema of [`MCP_CAPTURE_TERMINAL_TOOL`].
///
/// Closed on purpose, like the other read schemas: the terminal is named by
/// the id `devboule_list_terminals` answered, and `lines` is optional. There
/// is deliberately no workspace, owner or path parameter — the scope is the
/// calling session's own row, and a caller cannot widen it by stating one.
/// The parser reads this document back for its known-parameter check, so a
/// parameter can never be described here and unchecked there.
#[cfg(feature = "server")]
pub(crate) fn terminal_capture_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "terminalId": {
                "type": "string",
                "description": "The id of one terminal from devboule_list_terminals."
            },
            "lines": {
                "type": "integer",
                "minimum": 1,
                "maximum": 200,
                "description": "How many of the grid's bottom lines to return. Default 40, max 200."
            }
        },
        "required": ["terminalId"],
        "additionalProperties": false
    })
}

/// Which providers can be served the broker's tools, keyed by catalog id.
///
/// S9: all four session families host carriers now — ACP and Claude as before,
/// plus `codex` (`-c mcp_servers` overrides on the launch line, S6) and `pi`
/// (RPC bridge, S5) with post-spawn verification (S7/S8). A provider absent
/// from this table advertises no tools — the panel then hides its tool
/// section, because there is nothing there to gate.
pub const AGENT_MCP_TOOLS: &[(&str, &[(&str, &str)])] = &[
    ("claude", MCP_BROKER_TOOLS),
    ("codex", MCP_BROKER_TOOLS),
    ("gemini", MCP_BROKER_TOOLS),
    ("grok", MCP_BROKER_TOOLS),
    ("pi", MCP_BROKER_TOOLS),
    ("qwen", MCP_BROKER_TOOLS),
];

/// The broker tools `agent_id` is served, in catalog order. Unknown ids (a
/// registry wrapper, or a provider that cannot host the broker) get none.
pub fn mcp_tools_for(agent_id: &str) -> Vec<devboule_protocol::ToolDescriptor> {
    AGENT_MCP_TOOLS
        .iter()
        .find(|(id, _)| id.eq_ignore_ascii_case(agent_id))
        .map(|(_, tools)| {
            tools
                .iter()
                .map(|(name, description)| devboule_protocol::ToolDescriptor {
                    name: (*name).to_string(),
                    description: (*description).to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The session kind a provider's sessions are created as.
///
/// One match, in the catalog, because the two must agree: `claude` is the
/// stream-json kind, `codex` the app-server kind, `pi` the RPC kind, and every
/// other provider a created session may name is ACP. `resolve_session_provider`
/// reads the pair back and would refuse a `claude` sent as ACP.
///
/// This is also the **one place a provider name is consulted on the
/// `unattended` marker's path** (audit R2b-1 §5.1): the birth, the consent
/// card and `list_profiles` all resolve the kind here before
/// `peer_policy::unattended_mode` judges the delivered mode against it, so
/// the derivation does depend on a provider-name match — what it never does
/// is *interpret* one. The default arm is the conservative one for the
/// marker: a name that is not one of the three authored families is ACP,
/// whose arm answers `unknown`. The match is case-sensitive, unlike its
/// neighbour [`catalog_provider_id`], which is documented case-insensitive:
/// a profile spelling its provider `Claude` resolves the catalog row but
/// derives under ACP and answers `unknown` where `claude` would answer the
/// family's own value. That direction is the safe one (an uncertainty, never
/// a false certainty), and the asymmetry is spelled out here so the next
/// reader does not take either behaviour for the other's.
#[cfg(feature = "server")]
pub(crate) fn session_kind_for(provider: &str) -> devboule_protocol::SessionKind {
    use devboule_protocol::SessionKind;
    match provider {
        "claude" => SessionKind::Claude,
        "codex" => SessionKind::Codex,
        "pi" => SessionKind::Pi,
        _ => SessionKind::Acp,
    }
}

/// May a preset ever resolve to this mode, for this provider (`S5` decision 2)?
///
/// A description of today's providers, and a description only: the modes the
/// two named providers document as unattended, spelled as they spell them. It
/// serves the preset table's property tests and nothing else — it is **not**
/// what decides a permission prompt at run time (that is
/// `PermissionBroker::auto_answer`, which honours exactly the three
/// provider-agnostic ids [`mode_is_auto_answered`] lists), and **nothing new
/// may be derived from it** (rev 11: the provider axis is open, so no new
/// table, `match` or constant may grow from a list of provider or mode names —
/// the next providers are queued and some will be user-defined).
///
/// Written as a refusal list rather than an allow list, and the reason is the
/// decision's own: the modes a session nobody is watching may not be in have
/// names, and a new mode a provider adds is judged by them rather than by
/// whether someone remembered to add it to a table.
///
/// Three sources, all named:
///
/// - the three provider-agnostic ids the daemon itself answers a permission
///   request in ([`mode_is_auto_answered`]);
/// - Codex's own unattended pair: `full-access` is `approvalPolicy: never`, and
///   `auto-review` hands approvals to a model reviewer;
/// - Claude's `acceptEdits`, which approves every edit tool without prompting,
///   and its `auto`, which is a model-reviewed approvals mode — the same act
///   Codex spells `auto-review`.
///
/// Codex's `auto` is deliberately **not** on the list: it is `on-request` plus
/// `workspaceWrite`, which is exactly the mode decision 2 allows. Aliases
/// resolve first, so an alias cannot reach a mode its provider's own spelling
/// would refuse.
#[cfg(test)]
pub(crate) fn mode_is_unattended(provider: &str, mode_id: &str) -> bool {
    const CODEX_UNATTENDED: &[&str] = &["full-access", "auto-review"];
    const CLAUDE_UNATTENDED: &[&str] = &["acceptEdits", "auto"];
    if mode_is_auto_answered(mode_id) {
        return true;
    }
    match catalog_provider_id(provider).as_deref() {
        Some("codex") => CODEX_UNATTENDED.contains(&mode_id),
        Some("claude") => CLAUDE_UNATTENDED.contains(&mode_id),
        _ => false,
    }
}

/// The mode ids the daemon itself answers a permission request in, and the
/// one list of them.
///
/// Two callers, one list, so the two cannot drift: `PermissionBroker::
/// auto_answer` grants from this predicate at run time — a session in one of
/// these modes never asks a human — and `peer_policy::unattended_mode` checks
/// it first at a child's birth, so the marker a child carries names exactly a
/// mode the broker would have honoured. The list is provider-agnostic by
/// construction: these are the ids the daemon speaks itself, and no provider
/// name is reachable from here.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub(crate) fn auto_answered_modes() -> &'static [&'static str] {
    &["bypass", "auto_accept", "bypassPermissions"]
}

/// The client-only build answers no permission requests, so this has no
/// caller there — the gate that reads it is server-only by definition.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub(crate) fn mode_is_auto_answered(mode_id: &str) -> bool {
    auto_answered_modes().contains(&mode_id)
}

/// The pre-card verdict on an `autoAccept` tick against the profile's own
/// mode, split by who owns the mode id (`R2a` F7, narrowed by the re-audit's
/// P1).
///
/// The distinction is **authorship**, and it is the same one
/// `peer_policy::unattended_mode` draws for the birth marker: a mode id this
/// daemon did not author is a fact this daemon cannot state, and a gate that
/// refuses must not guess in the refusing direction. The conviction was
/// Codex's `full-access` — provider-authored vocabulary, `approvalPolicy:
/// never`, never asks anybody — which a `!mode_is_auto_answered` shape
/// refused before the card while Codex's own client accepted it at spawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(feature = "server")]
pub(crate) enum AutoAcceptTick {
    /// The daemon knows the pair is consistent: the tick is off, or the mode
    /// is one of the provider-agnostic ids [`mode_is_auto_answered`] lists —
    /// the one piece of mode vocabulary the daemon owns, and one every
    /// family's spawn-time check honours.
    Consistent,
    /// The daemon owns this family's tick rule and the profile violates it,
    /// so the creation is refused before the card. Two families only: for
    /// Claude and Pi the tick's meaning **is** "start in a mode the daemon's
    /// own broker answers" (the launch flag and the permission extension are
    /// the mechanisms), the profile's mode is the delivered mode — no
    /// handshake negotiates it — and each client's spawn-time check is
    /// exactly this predicate. The rule reads only the daemon's table, so
    /// the refusal is on vocabulary the daemon owns even though the mode id
    /// it refuses may be the family's own spelling.
    Contradicts,
    /// The mode id is not the daemon's, so whether the tick contradicts is
    /// not the daemon's fact to state: the family that owns the knob judges
    /// at spawn time, where the delivered mode is the fact. Codex's rule is
    /// its own (`codex_client::mode_answers_own_prompts` extends the daemon's
    /// table with `full-access`), an ACP agent's modes are authored at
    /// runtime and judged post-handshake, and a user-defined provider —
    /// which resolves to the ACP family — fails safe the same way: it is
    /// judged by the client that will speak for it, never refused by a
    /// table that never heard of it.
    NotOursToJudge,
}

/// The pre-card judgement of an `autoAccept` tick against the profile's own
/// mode (`R2a` F7, narrowed by the re-audit's P1 into the authorship split
/// [`AutoAcceptTick`] names).
///
/// [`mode_is_auto_answered`] is still the only mode table this reads. What
/// changed is the conclusion a non-table id licenses: the old shape read
/// "not broker-answered" as "asks the human" — a fact about vocabulary the
/// daemon did not author, and wrong for exactly the modes providers spell
/// for never-asking. The broker refuses only on [`AutoAcceptTick::Contradicts`];
/// every other family re-judges at spawn time, where the *delivered* mode is
/// the fact.
#[cfg(feature = "server")]
pub(crate) fn judge_auto_accept_tick(
    provider: &str,
    mode_id: &str,
    features: &serde_json::Map<String, serde_json::Value>,
) -> AutoAcceptTick {
    if !crate::profile_delivery::feature_is_true(features, AUTO_ACCEPT_FEATURE) {
        return AutoAcceptTick::Consistent;
    }
    if mode_is_auto_answered(mode_id) {
        return AutoAcceptTick::Consistent;
    }
    match session_kind_for(provider) {
        devboule_protocol::SessionKind::Claude | devboule_protocol::SessionKind::Pi => {
            AutoAcceptTick::Contradicts
        }
        // Every arm spelled, no wildcard: a family added to the closed
        // `SessionKind` must be decided here, not inherit the benign
        // verdict. The three below all answer "not ours to judge" — Codex
        // owns its knob (`codex_client::mode_answers_own_prompts` extends
        // the daemon's table with `full-access`), an ACP agent's modes are
        // authored at runtime and judged post-handshake, and a terminal has
        // no permission mechanism for a tick to contradict at all.
        // `session_kind_for` maps every name that is not one of the three
        // authored families to `Acp`, so `Terminal` is unreachable through
        // this door today — it is decided, not defaulted.
        devboule_protocol::SessionKind::Acp
        | devboule_protocol::SessionKind::Codex
        | devboule_protocol::SessionKind::Terminal => AutoAcceptTick::NotOursToJudge,
    }
}

/// The one feature key that means "approve my permission prompts"
/// (`create-from-profile`).
///
/// Paseo spells the toggle `Auto Accept`, and this is the only spelling read:
/// the features map is otherwise free-form and nothing consults it, so a second
/// accepted spelling would be a second vocabulary for one meaning.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub(crate) const AUTO_ACCEPT_FEATURE: &str = "autoAccept";

/// The exact id the catalog publishes for `agent_id`, when it publishes tools
/// under any spelling of it.
///
/// The row match above is case-insensitive — a caller that spells a provider
/// `CLAUDE` is naming the same provider — while everything keyed by a provider
/// id (the tool policy file, `ProviderInfo.tools`) is keyed by the exact string.
/// That gap is C-1: an id admitted case-insensitively and then looked up exactly
/// is admitted and never found. Callers that store or compare an id resolve it
/// here first, so the admission check and the later lookup cannot disagree on
/// case. The predicate is the same `AGENT_MCP_TOOLS` match `mcp_tools_for`
/// performs, and `the_canonical_id_agrees_with_the_tool_lookup` pins the two
/// together.
pub fn mcp_catalog_id(agent_id: &str) -> Option<&'static str> {
    AGENT_MCP_TOOLS
        .iter()
        .find(|(id, _)| id.eq_ignore_ascii_case(agent_id))
        .map(|(id, _)| *id)
}

/// Equality is on the deny set, not the source tag: a preset list and a
/// profile list with the same names allow the same tools, so a live child
/// and a resumed one compare equal. Order-free, like `allows`.
#[cfg(feature = "server")]
impl PartialEq for ToolOverlay {
    fn eq(&self, other: &Self) -> bool {
        let mut own = self.disabled_names();
        let mut theirs = other.disabled_names();
        own.sort();
        theirs.sort();
        own == theirs
    }
}

/// The tool-policy overlay a creation applies to the sessions it makes
/// (`S5` §2).
///
/// An overlay can only ever *remove* tools. There is no field that grants one,
/// because a preset that widened a session's tools would be a second policy
/// authority beside the stored `ToolPolicyEntry` the human edits; the overlay
/// is the preset's own deny list, and the effective answer for one tool is
/// "the stored policy allows it AND the overlay allows it".
#[derive(Clone, Debug, Eq)]
#[cfg(feature = "server")]
pub(crate) struct ToolOverlay {
    /// The tool names this overlay removes, from whichever source made it.
    disabled: OverlayNames,
}

/// Where an overlay's deny list comes from.
///
/// Two sources, one type, because both have to be consulted at the same two
/// places (`tools/list` and `tools/call`): a preset's own table, which the
/// catalog's constants hold and which must stay `const` for the preset table to
/// be one, and a profile the human wrote — read from the store at the moment of
/// the creation and therefore owned by the value that resolved it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(feature = "server")]
enum OverlayNames {
    /// A preset's list, borrowed from the catalog's own table.
    Preset(&'static [&'static str]),
    /// A profile's list, exactly as the store holds it. `agent_profiles.rs`
    /// has already refused any name outside the broker's own table, so a name
    /// here can only ever match a tool the broker serves.
    Profile(Vec<String>),
}

#[cfg(feature = "server")]
impl Default for ToolOverlay {
    fn default() -> Self {
        Self::NONE
    }
}

#[cfg(feature = "server")]
impl ToolOverlay {
    /// The empty overlay: the session is served exactly what its provider's
    /// stored policy allows.
    pub(crate) const NONE: Self = Self {
        disabled: OverlayNames::Preset(&[]),
    };
    /// A design child: no `devboule_send_message`, no `devboule_create_agent`,
    /// no `devboule_create_workspace`, and none of the supervision verbs —
    /// `devboule_cancel_agent`, `devboule_stop_agent`, `devboule_close_agent`.
    /// It keeps the roster, which is its own bearer's read-only view. Depth
    /// alone would not stop it (a depth-1 child may create), so the deny list
    /// is the rule.
    pub(crate) const DESIGN: Self = Self {
        disabled: OverlayNames::Preset(&[
            MCP_SEND_MESSAGE_TOOL,
            MCP_CREATE_AGENT_TOOL,
            MCP_CREATE_WORKSPACE_TOOL,
            MCP_CANCEL_AGENT_TOOL,
            MCP_STOP_AGENT_TOOL,
            MCP_CLOSE_AGENT_TOOL,
        ]),
    };

    /// A profile's overlay: the tools the human's profile denies, by name.
    /// Normalised (sorted, deduped) so equality stays order- and
    /// repetition-free like `allows`: the store does not refuse duplicates,
    /// and a live overlay must compare equal to its resumed twin.
    pub(crate) fn from_profile_names(names: &[String]) -> Self {
        let mut names = names.to_vec();
        names.sort();
        names.dedup();
        Self {
            disabled: OverlayNames::Profile(names),
        }
    }

    /// The deny list as owned names, for the journal column. The source
    /// (a preset const vs a profile vec) is behavior-free — `allows` reads
    /// names only — so the column keeps names alone and the read rebuilds
    /// the owned form; a live child and a resumed one compare equal.
    pub(crate) fn disabled_names(&self) -> Vec<String> {
        match &self.disabled {
            OverlayNames::Preset(names) => names.iter().map(|name| name.to_string()).collect(),
            OverlayNames::Profile(names) => names.clone(),
        }
    }

    /// The names this overlay denies, for the tests that must see them.
    ///
    /// A view, not a field: the two sources hold their names differently, and a
    /// test that walked `disabled` directly would be testing the representation
    /// rather than the rule.
    #[cfg(test)]
    pub(crate) fn denied(&self) -> Vec<&str> {
        match &self.disabled {
            OverlayNames::Preset(names) => names.to_vec(),
            OverlayNames::Profile(names) => names.iter().map(String::as_str).collect(),
        }
    }

    pub(crate) fn allows(&self, name: &str) -> bool {
        match &self.disabled {
            OverlayNames::Preset(names) => !names.contains(&name),
            OverlayNames::Profile(names) => !names.iter().any(|denied| denied == name),
        }
    }
}

/// One provider's row in a preset: the mode the child is created in and the
/// overlay applied to it (`S5` §2).
///
/// A cell is a *promise* about a provider, and the daemon keeps it only where
/// it can: the mode is applied through the provider's own mode switch, and the
/// overlay only reaches a provider whose sessions are given an MCP connection
/// at all. S9: all agent families host carriers, so pi/codex deny-cells are
/// enforced live (a `design` Codex child sees neither the tool it may not call
/// nor any tool its provider's policy already turned off) — the same rule the
/// ACP families always had.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(feature = "server")]
pub(crate) struct AgentPresetCell {
    /// Catalog id, exactly as [`KNOWN_AGENTS`] spells it.
    pub(crate) provider: &'static str,
    pub(crate) mode: &'static str,
    pub(crate) overlay: ToolOverlay,
}

/// One preset: its closed set of provider cells and the preamble every session
/// it creates is given (`S5` §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(feature = "server")]
pub(crate) struct AgentPreset {
    pub(crate) id: &'static str,
    pub(crate) preamble: &'static str,
    pub(crate) cells: &'static [AgentPresetCell],
}

// Kept as data, not as a path (`BRIEF-slice-5.md` §2 rev 9): a profile is now the
// only thing a creation resolves, and these rows survive as the **seed values**
// the Settings → Agents form offers and as the two overlays it starts from. No
// production caller resolves one, which is what `#[allow(dead_code)]` says; the
// slice that publishes the seeds to the app is what removes this block.
#[allow(dead_code)]
#[cfg(feature = "server")]
pub(crate) const AGENT_PRESET_WORKER: &str = "worker";
#[cfg(feature = "server")]
pub(crate) const AGENT_PRESET_DESIGN: &str = "design";

/// The one preamble both presets carry.
///
/// It asks for the result in the final message and says nothing about files:
/// the finish hook deposits that message by itself (§3, decision 10), so a
/// preamble that told the child to write files would be asking for the same
/// artifact twice, in the one place a child can put it out of reach.
/// `preambles_hold_no_instruction_to_write_files` pins the property.
#[cfg(feature = "server")]
pub(crate) const AGENT_PREAMBLE: &str =
    "You were created by another agent; report your result in your final message.";

/// The `worker` cells: the allowed mode of each catalog provider, no overlay.
///
/// The modes are the ones decision 2 measured as allowed — pi `ask` (the only
/// non-`bypass` mode pi has), Codex `auto` (`on-request` + `workspaceWrite`,
/// no network), Claude `default`, ACP `default` for the ACP providers the
/// catalog publishes. `devboule-acp-stub` is deliberately absent: it is a
/// debug-only health probe that implements no `session/set_mode`, so a
/// creation naming it is refused with `no non-bypass mode for provider`.
#[cfg(feature = "server")]
#[allow(dead_code)]
const PRESET_WORKER_CELLS: &[AgentPresetCell] = &[
    AgentPresetCell {
        provider: "pi",
        mode: "ask",
        overlay: ToolOverlay::NONE,
    },
    AgentPresetCell {
        provider: "codex",
        mode: "auto",
        overlay: ToolOverlay::NONE,
    },
    AgentPresetCell {
        provider: "claude",
        mode: "default",
        overlay: ToolOverlay::NONE,
    },
    AgentPresetCell {
        provider: "grok",
        mode: "default",
        overlay: ToolOverlay::NONE,
    },
    AgentPresetCell {
        provider: "qwen",
        mode: "default",
        overlay: ToolOverlay::NONE,
    },
    AgentPresetCell {
        provider: "gemini",
        mode: "default",
        overlay: ToolOverlay::NONE,
    },
];

/// The `design` cells: the same modes, with the design overlay.
///
/// There is no Design preamble to copy and none is invented here: the Design
/// instructions are composed per request by the app and travel in
/// `initialPrompt`. A frozen copy in the catalog would be a second source of
/// truth for a prompt the app still owns.
#[cfg(feature = "server")]
#[allow(dead_code)]
const PRESET_DESIGN_CELLS: &[AgentPresetCell] = &[
    AgentPresetCell {
        provider: "pi",
        mode: "ask",
        overlay: ToolOverlay::DESIGN,
    },
    AgentPresetCell {
        provider: "codex",
        mode: "auto",
        overlay: ToolOverlay::DESIGN,
    },
    AgentPresetCell {
        provider: "claude",
        mode: "default",
        overlay: ToolOverlay::DESIGN,
    },
    AgentPresetCell {
        provider: "grok",
        mode: "default",
        overlay: ToolOverlay::DESIGN,
    },
    AgentPresetCell {
        provider: "qwen",
        mode: "default",
        overlay: ToolOverlay::DESIGN,
    },
    AgentPresetCell {
        provider: "gemini",
        mode: "default",
        overlay: ToolOverlay::DESIGN,
    },
];

/// The closed preset table (`S5` §2). A preset not in this list is an unknown
/// preset and is refused by name rather than resolved by default.
#[cfg(feature = "server")]
#[allow(dead_code)]
pub(crate) const AGENT_PRESETS: &[AgentPreset] = &[
    AgentPreset {
        id: AGENT_PRESET_WORKER,
        preamble: AGENT_PREAMBLE,
        cells: PRESET_WORKER_CELLS,
    },
    AgentPreset {
        id: AGENT_PRESET_DESIGN,
        preamble: AGENT_PREAMBLE,
        cells: PRESET_DESIGN_CELLS,
    },
];

/// Every provider row the catalog publishes, in catalog order, test-only
/// rows included because a debug daemon serves them. This is the enumeration
/// the provider registry binds its implementations through (`provider.rs`):
/// the catalog stays the one list of names.
#[cfg(feature = "server")]
pub(crate) fn catalog_provider_rows() -> impl Iterator<Item = &'static KnownAgent> {
    KNOWN_AGENTS
        .iter()
        // The test-only rows are part of the catalog a debug build serves
        // (audit S5B-08); a release build's list is empty.
        .chain(TEST_ONLY_AGENTS.iter())
}

/// The provider ids the catalog publishes, in catalog order, test-only rows
/// included because a debug daemon serves them. Ids only — the set a user row
/// may not take is [`catalog_reserved_names`], which is these **and** the
/// aliases.
#[cfg(test)]
fn catalog_provider_ids() -> Vec<&'static str> {
    KNOWN_AGENTS
        .iter()
        // The test-only rows are part of the catalog a debug build serves
        // (audit S5B-08): they carry preset cells, so the property below has to
        // walk them too. In a release build this list is empty.
        .chain(TEST_ONLY_AGENTS.iter())
        .map(|agent| agent.id)
        .collect()
}

/// Every name the catalog answers to — ids **and** aliases — which is the set
/// a user row may not take. Ids alone is the wrong set: several names are
/// alias-only (`claude-code`, `grok-build`, `qwen-code`, and the debug rows),
/// and a row taking one of those would be refused by nobody while
/// [`catalog_provider_id`] still canonicalised it to the built-in — one name
/// with two answers, because `acp_client::resolve_named` consults the user
/// rows *before* the catalog walk. The shadow the refusal exists to prevent
/// arrives through the door the refusal did not cover.
#[cfg(feature = "server")]
pub(crate) fn catalog_reserved_names() -> Vec<&'static str> {
    KNOWN_AGENTS
        .iter()
        // Same reason as [`catalog_provider_ids`]: a debug daemon serves the
        // test-only rows, so their names are reserved in a debug build too.
        .chain(TEST_ONLY_AGENTS.iter())
        .flat_map(|agent| std::iter::once(agent.id).chain(agent.aliases.iter().copied()))
        .collect()
}

/// The catalog's own spelling of `agent_id`, when it publishes the id or an
/// alias of it — now owned, because the catalogue is no longer compile-time
/// closed: a user row that is live in the registry snapshot canonicalises
/// like any other provider id.
///
/// The walk is the snapshot's **published** ids — the entries, i.e. rows
/// that bind an implementation and can spawn — never the raw `user_rows`
/// alone: a profile cannot name a provider nothing can spawn (pass 2e's
/// ordering rule, enforced here and pinned by
/// `a_profile_lookup_answers_for_rows_that_bind_an_implementation_only` in
/// `provider.rs`'s tests). Built-in ids and aliases answer first, exactly as
/// before this pass; a live user row answers only where nothing matched
/// before.
///
/// `mcp_catalog_id` answers only for MCP-capable providers, which is the wrong
/// set here: a profile may name any installed provider, so this walks the whole
/// catalog instead.
#[cfg(feature = "server")]
pub(crate) fn catalog_provider_id(agent_id: &str) -> Option<String> {
    catalog_provider_id_for(&crate::session::catalog_registry(), agent_id)
}

/// The lookup against one snapshot: the built-ins' ids and aliases first
/// (the static walk, unchanged), then the snapshot's published ids. Takes
/// the whole snapshot — and answers from `published_ids` alone — so the
/// ordering rule above is testable on a local registry where a row is
/// declared but does not bind an implementation.
#[cfg(feature = "server")]
pub(crate) fn catalog_provider_id_for(
    registry: &crate::session::ProviderRegistry,
    agent_id: &str,
) -> Option<String> {
    if let Some(agent) = KNOWN_AGENTS
        .iter()
        // The test-only rows are part of the catalog a *debug* daemon serves
        // ([`TEST_ONLY_AGENTS`] is empty in a release build), and a preset cell
        // for one of them resolves through the same door as any other provider
        // id: without this, a debug daemon would publish a provider it then
        // refused as unknown.
        .chain(TEST_ONLY_AGENTS.iter())
        .find(|agent| {
            agent.id.eq_ignore_ascii_case(agent_id)
                || agent
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(agent_id))
        })
    {
        return Some(agent.id.to_string());
    }
    registry
        .published_ids()
        .find(|id| id.eq_ignore_ascii_case(agent_id))
        .map(|id| id.to_string())
}

/// The preset with this id, or `None`. Case-sensitive: a preset id is our own
/// closed vocabulary, not a user's spelling.
#[cfg(feature = "server")]
#[allow(dead_code)]
pub(crate) fn agent_preset(preset_id: &str) -> Option<&'static AgentPreset> {
    AGENT_PRESETS.iter().find(|preset| preset.id == preset_id)
}

/// Resolve one `(preset, provider)` cell, or the sentence that refuses it.
///
/// The refusals are decision 2's, in the order the caller can fix them: the
/// preset is ours, the provider is the catalog's, and the mode is the one the
/// catalog says that provider runs watched by a human. A provider the catalog
/// publishes but the preset does not cover has no allowed mode, which is a
/// different sentence from an unknown provider — the first is a limit of this
/// version, the second is a typo.
#[cfg(feature = "server")]
#[allow(dead_code)]
pub(crate) fn resolve_agent_preset(
    preset_id: &str,
    provider_id: &str,
) -> Result<(&'static AgentPreset, AgentPresetCell), String> {
    let Some(preset) = agent_preset(preset_id) else {
        return Err("unknown preset".to_string());
    };
    let Some(provider) = catalog_provider_id(provider_id) else {
        return Err("unknown provider".to_string());
    };
    if let Some(cell) = preset
        .cells
        .iter()
        .find(|cell| cell.provider == provider.as_str())
        .cloned()
    {
        return Ok((preset, cell));
    }
    // A debug build also accepts the test-only providers, and only there: the
    // release table above never names them (`S5` e2e). The seam is one function
    // so its surface is one thing to audit.
    #[cfg(debug_assertions)]
    if let Some(cell) = test_only_cell(preset.id, &provider) {
        return Ok((preset, cell));
    }
    Err("no non-bypass mode for provider".to_string())
}

/// The cells a **debug** build accepts for the test-only providers, and the
/// whole of the seam they need (`S5` block 6).
///
/// `devboule-acp-stub` is the integration stub binary: it is the only provider
/// in this tree that implements `session/set_mode`, so it is what makes a child
/// creation measurable end to end, including the mode the preset cell names.
/// `devboule-absent-probe` names a binary that cannot exist anywhere, and is
/// how `provider not installed` is reached without depending on what happens to
/// be installed on the machine running the tests.
///
/// Neither is in [`AGENT_PRESETS`]: the release table has six cells per preset
/// (`the_release_table_never_names_a_test_provider`), the release build has no
/// `test_only_cell` at all, and a release daemon therefore refuses both with
/// `unknown provider`.
#[cfg(all(feature = "server", debug_assertions))]
#[allow(dead_code)]
fn test_only_cell(preset_id: &str, provider: &str) -> Option<AgentPresetCell> {
    let cell = |provider: &'static str, overlay: ToolOverlay| {
        Some(AgentPresetCell {
            provider,
            mode: "default",
            overlay,
        })
    };
    match (preset_id, provider) {
        (AGENT_PRESET_WORKER, "devboule-acp-stub") => cell("devboule-acp-stub", ToolOverlay::NONE),
        (AGENT_PRESET_DESIGN, "devboule-acp-stub") => {
            cell("devboule-acp-stub", ToolOverlay::DESIGN)
        }
        (AGENT_PRESET_WORKER, "devboule-absent-probe") => {
            cell("devboule-absent-probe", ToolOverlay::NONE)
        }
        (AGENT_PRESET_DESIGN, "devboule-absent-probe") => {
            cell("devboule-absent-probe", ToolOverlay::DESIGN)
        }
        _ => None,
    }
}

/// The overlays in play, for the tests that must see every one of them.
#[cfg(test)]
pub(crate) fn overlay_names() -> Vec<ToolOverlay> {
    AGENT_PRESETS
        .iter()
        .flat_map(|preset| preset.cells.iter().map(|cell| cell.overlay.clone()))
        .collect()
}

/// Registry wrappers that a better native chat-capable provider covers in the
/// workspace picker. This is an explicit product-policy map from §1.1:
/// `claude-acp` is a proprietary npx wrapper, while native `claude` already
/// speaks stream-json and reuses the user's Claude subscription. It stays in
/// Settings so the installed option remains honest and discoverable.
/// `codex-acp` is covered because native codex is the first-class Codex
/// chat provider. `pi-acp` is covered because native pi is the first-class Pi
/// chat provider. The native `pi-acp` 0.0.33 wrapper speaks ACP and reports
/// models, but does not emit `session/request_permission` for native tools;
/// a measured write completed with zero permission requests. The wrapper
/// therefore stays visible in Settings and is not pickable when native Pi is
/// installed.
#[cfg(feature = "server")]
const REGISTRY_NATIVE_CHAT_COVERAGE: &[(&str, &str)] = &[
    ("claude-acp", "claude"),
    ("codex-acp", "codex"),
    ("pi-acp", "pi"),
];

/// Explicit product policy for selecting the default ACP provider.
///
/// This is deliberately separate from `KNOWN_AGENTS` layout: on 2026-09-04,
/// grok was the only ACP agent verified to complete a working session on this
/// machine; qwen completed the handshake but declares its ACP flag deprecated.
const ACP_PREFERENCE: &[&str] = &["grok", "qwen", "gemini"];

/// Authentication is intentionally not probed by the catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationStatus {
    Unknown,
}

/// How a catalog row was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderOrigin {
    UserBinary,
    NpxWrapper,
}

impl ProviderOrigin {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::UserBinary => "user-binary",
            Self::NpxWrapper => "npx-wrapper",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallChannel {
    Npm,
    NpxRegistry,
    Native,
}

impl InstallChannel {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::NpxRegistry => "npx-registry",
            Self::Native => "native",
        }
    }
}

/// Direct CreateProcess program plus arguments that must precede ACP/user args.
///
/// A native CLI (`claude.exe`, `grok.exe`) has an empty `prefix_args`. An
/// unwrapped npm cmd-shim is `node` plus the package script path.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolvedLaunch {
    program: PathBuf,
    prefix_args: Vec<String>,
    /// The PATH directory the launch was resolved from: the shim's directory
    /// when a cmd-shim was unwrapped to `node`, the search directory
    /// otherwise. The spawned program's own parent is not enough — after an
    /// unwrap the parent is node's, and the provider's folder is the one the
    /// child still needs on its PATH.
    source_directory: PathBuf,
}

impl ResolvedLaunch {
    /// Test-only constructor: a bare direct program, resolved from its own
    /// parent directory.
    #[cfg(test)]
    fn program(program: PathBuf) -> Self {
        let source_directory = program.parent().unwrap_or(Path::new("")).to_path_buf();
        Self {
            program,
            prefix_args: Vec::new(),
            source_directory,
        }
    }

    fn launched(program: PathBuf, prefix_args: Vec<String>, source_directory: PathBuf) -> Self {
        Self {
            program,
            prefix_args,
            source_directory,
        }
    }
}

/// One known provider found on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledAgent {
    pub id: String,
    pub aliases: &'static [&'static str],
    /// A synthetic known npm row is visible to provider settings but must
    /// never be treated as a launchable session provider.
    pub installed: bool,
    pub executable: PathBuf,
    /// Arguments inserted between `executable` and ACP/user args.
    /// Empty for a native binary; the package script for an npm cmd-shim.
    pub prefix_args: Vec<String>,
    /// Resolved spawn argv for ACP: executable, then prefix args, then ACP
    /// flags. Element 0 is the path that CreateProcess will run.
    pub acp_command: Option<Vec<String>>,
    /// Resolved spawn argv for Claude stream-json: executable, then prefix
    /// args, then the measured flags. Element 0 is the path that CreateProcess
    /// will run.
    pub stream_json_command: Option<Vec<String>>,
    /// Resolved spawn argv for Pi RPC: executable, then prefix args, then the
    /// Pi RPC flags. Element 0 is the path that CreateProcess will run.
    pub rpc_command: Option<Vec<String>>,
    /// Resolved spawn argv for Codex app-server: executable, then prefix
    /// args, then the app-server flag.
    pub app_server_command: Option<Vec<String>>,
    pub authentication: AuthenticationStatus,
    pub origin: ProviderOrigin,
    /// Registry-supplied arguments appended after `npx -y <package>`. None
    /// for native providers; Some, including an empty vector, for wrappers.
    pub launch_args: Option<Vec<String>>,
    /// Explicit picker policy. Covered wrappers are kept in Settings but are
    /// omitted from the workspace provider picker.
    pub pickable: Option<bool>,
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
    pub install_channel: InstallChannel,
    pub npm_package: Option<&'static str>,
    /// Tools the daemon's MCP broker serves to this provider's sessions, from
    /// [`AGENT_MCP_TOOLS`]. Empty for a provider with no MCP channel: the row
    /// then carries no `tools` key on the wire and the panel hides the tool
    /// section rather than offering toggles with nothing behind them.
    pub tools: Vec<devboule_protocol::ToolDescriptor>,
    /// The PATH env pair this row's spawn must carry when the executable's
    /// folder is named only by the registry PATH the inherited PATH predates
    /// (Windows; `None` otherwise, including every row of the injected
    /// `*_in_paths` seam, which stay pure). Never serialized to the wire.
    pub(crate) spawn_path_env: Option<(String, String)>,
    /// The PATH directory the launch was resolved from: the shim's directory
    /// for an unwrapped cmd-shim, npx's directory for a registry wrapper row.
    /// `None` for synthetic rows; the deciding spawn policy reads it, not the
    /// wire.
    pub(crate) launch_directory: Option<PathBuf>,
}

/// PATH scan result. `unreadable_dirs` is the number of unique PATH entries
/// that could not be listed (I/O error, not "the directory is missing").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderDiscovery {
    pub agents: Vec<InstalledAgent>,
    pub unreadable_dirs: u32,
}

/// Return the known agents whose executable can be resolved from PATH.
pub fn discover() -> ProviderDiscovery {
    #[cfg(windows)]
    {
        discover_with_path_source(&crate::windows_path_env::RegistryPathSource)
    }
    #[cfg(not(windows))]
    {
        discover_in_paths(&discovery_directories())
    }
}

/// The directories discovery searches. On Windows the daemon's inherited PATH
/// can predate folders the machine or user PATH registry values name (the
/// launcher's login-time environment), so discovery merges those entries in
/// behind the process PATH; on unix the login-shell capture already refreshed
/// the process PATH and nothing is added.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub(crate) fn discovery_directories() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        crate::windows_path_env::PathSnapshot::capture(&crate::windows_path_env::RegistryPathSource)
            .directories()
    }
    #[cfg(not(windows))]
    {
        match std::env::var_os("PATH") {
            Some(paths) => std::env::split_paths(&paths).collect(),
            None => Vec::new(),
        }
    }
}

/// [`discover`] with the environment behind the seam: the exact discovery
/// path the daemon runs, driven from an injected source, so a test reproduces
/// a stale inherited PATH without touching the registry. Rows found in a
/// folder only the registry PATH names carry their spawn PATH override.
#[cfg(windows)]
pub(crate) fn discover_with_path_source(
    source: &dyn crate::windows_path_env::WindowsPathSource,
) -> ProviderDiscovery {
    let snapshot = snapshot_of(source);
    let mut discovery = discover_in_paths(&snapshot.directories());
    crate::windows_path_env::attach_spawn_path_env(&mut discovery.agents, &snapshot);
    discovery
}

/// Bind the real registry source for callers that discover with
/// [`path_directories`] directly (the refresh path) and still need rows to
/// carry their spawn PATH override. One registry capture for the whole call.
#[cfg_attr(any(not(windows), not(feature = "server")), allow(dead_code))]
pub(crate) fn attach_spawn_path_env(discovery: &mut ProviderDiscovery) {
    #[cfg(windows)]
    {
        let snapshot = snapshot_of(&crate::windows_path_env::RegistryPathSource);
        crate::windows_path_env::attach_spawn_path_env(&mut discovery.agents, &snapshot);
    }
}

/// The one environment capture a discovery or resolution pass works from.
#[cfg(windows)]
fn snapshot_of(
    source: &dyn crate::windows_path_env::WindowsPathSource,
) -> crate::windows_path_env::PathSnapshot {
    crate::windows_path_env::PathSnapshot::capture(source)
}

pub(crate) fn discover_in_paths(directories: &[PathBuf]) -> ProviderDiscovery {
    let agents = KNOWN_AGENTS
        .iter()
        // Chained here (the only KNOWN_AGENTS consumption in this module) so
        // every discovery path — ProvidersList, find_in_catalog,
        // find_available — sees the test-only row in debug builds.
        .chain(TEST_ONLY_AGENTS.iter())
        .filter_map(|spec| {
            let launch = spec
                .aliases
                .iter()
                .find_map(|alias| resolve_launch_command_in_paths(directories, alias))?;
            let acp_command = spec
                .acp_args
                .map(|args| protocol_argv(&launch.program, &launch.prefix_args, args));
            let stream_json_command = spec
                .stream_json_args
                .map(|args| protocol_argv(&launch.program, &launch.prefix_args, args));
            let rpc_command = spec
                .rpc_args
                .map(|args| protocol_argv(&launch.program, &launch.prefix_args, args));
            let app_server_command = spec
                .app_server_args
                .map(|args| protocol_argv(&launch.program, &launch.prefix_args, args));
            let install_channel = if launch.prefix_args.is_empty() {
                InstallChannel::Native
            } else {
                InstallChannel::Npm
            };
            let installed_version = launch
                .prefix_args
                .first()
                .map(PathBuf::from)
                .and_then(|script| package_json_version_from_script(&script));
            Some(InstalledAgent {
                id: spec.id.to_string(),
                aliases: spec.aliases,
                installed: true,
                executable: launch.program,
                prefix_args: launch.prefix_args,
                acp_command,
                stream_json_command,
                rpc_command,
                app_server_command,
                authentication: AuthenticationStatus::Unknown,
                origin: ProviderOrigin::UserBinary,
                launch_args: None,
                pickable: None,
                installed_version,
                latest_version: None,
                install_channel,
                npm_package: spec.npm_package,
                tools: mcp_tools_for(spec.id),
                spawn_path_env: None,
                launch_directory: Some(launch.source_directory)
                    .filter(|directory| !directory.as_os_str().is_empty()),
            })
        })
        .collect();
    ProviderDiscovery {
        agents,
        unreadable_dirs: count_unreadable_dirs(directories),
    }
}

fn protocol_argv(executable: &Path, prefix_args: &[String], extra: &[&str]) -> Vec<String> {
    let mut command = Vec::with_capacity(1 + prefix_args.len() + extra.len());
    command.push(executable.to_string_lossy().into_owned());
    command.extend(prefix_args.iter().cloned());
    command.extend(extra.iter().map(|arg| (*arg).to_string()));
    command
}

const MAX_PACKAGE_JSON_BYTES: u64 = 1024 * 1024;

/// Read the nearest npm package version above an unwrapped cmd-shim script.
/// The walk is bounded and stops at the containing node_modules directory.
pub(crate) fn package_json_version_from_script(script: &Path) -> Option<String> {
    let mut directory = script.parent()?.to_path_buf();
    if !directory.ancestors().any(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("node_modules"))
    }) {
        return None;
    }
    for _ in 0..32 {
        if directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("node_modules"))
        {
            break;
        }
        let package_json = directory.join("package.json");
        if let Ok(metadata) = std::fs::metadata(&package_json) {
            if metadata.is_file() && metadata.len() <= MAX_PACKAGE_JSON_BYTES {
                let body = std::fs::read_to_string(package_json).ok()?;
                return serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|value| value.get("version")?.as_str().map(str::to_string))
                    .and_then(|version| crate::provider_catalog::cap_external_version(&version));
            }
        }
        if !directory.pop() {
            break;
        }
    }
    None
}

fn count_unreadable_dirs(directories: &[PathBuf]) -> u32 {
    let mut seen = HashSet::new();
    let mut count = 0;
    for dir in directories {
        if dir.as_os_str().is_empty() || !seen.insert(dir.clone()) {
            continue;
        }
        match std::fs::read_dir(dir) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => count += 1,
        }
    }
    count
}

/// Chat dialect this installed agent can speak, if any.
/// An agent with both launches is offered as ACP: ACP is the road, stream-json the exception.
pub fn chat_protocol(agent: &InstalledAgent) -> Option<&'static str> {
    if !agent.installed {
        return None;
    }
    if agent.app_server_command.is_some() {
        Some("codex-app-server")
    } else if agent.acp_command.is_some() {
        Some("acp")
    } else if agent.stream_json_command.is_some() {
        Some("stream-json")
    } else if agent.rpc_command.is_some() {
        Some("pi-rpc")
    } else {
        None
    }
}

/// Return the first discovered provider that offers ACP.
pub fn first_acp_available() -> Option<InstalledAgent> {
    let discovered = discover();
    ACP_PREFERENCE.iter().find_map(|preferred_id| {
        discovered
            .agents
            .iter()
            .find(|agent| agent.id == *preferred_id && agent.acp_command.is_some())
            .cloned()
    })
}

/// Return the discovered provider with this catalog id, if it is on PATH.
pub fn find_available(id: &str) -> Option<InstalledAgent> {
    #[cfg(windows)]
    {
        find_available_with_path_source(id, &crate::windows_path_env::RegistryPathSource)
    }
    #[cfg(not(windows))]
    {
        find_available_in_paths(id, &discovery_directories())
    }
}

/// [`find_available`] with the environment behind the seam: the same
/// resolution the daemon runs, driven from an injected source. The returned
/// row carries its spawn PATH override when the registry named its folder.
#[cfg(windows)]
pub(crate) fn find_available_with_path_source(
    id: &str,
    source: &dyn crate::windows_path_env::WindowsPathSource,
) -> Option<InstalledAgent> {
    let snapshot = snapshot_of(source);
    let mut agent = find_available_in_paths(id, &snapshot.directories())?;
    crate::windows_path_env::attach_spawn_path_env(std::slice::from_mut(&mut agent), &snapshot);
    Some(agent)
}

pub(crate) fn find_available_in_paths(id: &str, directories: &[PathBuf]) -> Option<InstalledAgent> {
    discover_in_paths(directories)
        .agents
        .into_iter()
        .find(|agent| agent.installed && agent.id == id)
}

#[cfg(feature = "server")]
pub(crate) fn path_directories() -> Vec<PathBuf> {
    discovery_directories()
}

/// Local PATH scan plus ACP-registry npx rows. Native ids/aliases win.
#[cfg(feature = "server")]
pub fn discover_catalog(
    fetch: &dyn crate::registry::RegistryFetch,
    cache_dir: &Path,
) -> ProviderDiscovery {
    #[cfg(windows)]
    {
        let snapshot = snapshot_of(&crate::windows_path_env::RegistryPathSource);
        let mut discovery = discover_catalog_in_paths(fetch, cache_dir, &snapshot.directories());
        crate::windows_path_env::attach_spawn_path_env(&mut discovery.agents, &snapshot);
        discovery
    }
    #[cfg(not(windows))]
    {
        let mut discovery = discover_catalog_in_paths(fetch, cache_dir, &discovery_directories());
        attach_spawn_path_env(&mut discovery);
        discovery
    }
}

#[cfg(feature = "server")]
pub(crate) fn discover_catalog_in_paths(
    fetch: &dyn crate::registry::RegistryFetch,
    cache_dir: &Path,
    directories: &[PathBuf],
) -> ProviderDiscovery {
    let local = discover_in_paths(directories);
    let registry_entries = crate::registry::load_npx_entries(fetch, cache_dir);
    let local = add_missing_npm_rows(local, &registry_entries);
    let registry = registry_entries
        .into_iter()
        .map(|entry| registry_agent(directories, &local, entry))
        .collect();
    merge_native_beats_registry(local, registry)
}

#[cfg(feature = "server")]
fn add_missing_npm_rows(
    mut local: ProviderDiscovery,
    registry: &[crate::registry::RegistryNpxEntry],
) -> ProviderDiscovery {
    for spec in KNOWN_AGENTS
        .iter()
        .filter(|spec| spec.npm_package.is_some())
    {
        let local_has_row = local.agents.iter().any(|agent| agent.id == spec.id);
        let registry_covers_row = registry.iter().any(|entry| {
            entry.id.eq_ignore_ascii_case(spec.id)
                || spec
                    .aliases
                    .iter()
                    .any(|alias| entry.id.eq_ignore_ascii_case(alias))
        });
        if local_has_row || registry_covers_row {
            continue;
        }
        let package = spec.npm_package.expect("filtered npm package");
        local.agents.push(InstalledAgent {
            id: spec.id.to_string(),
            aliases: spec.aliases,
            installed: false,
            executable: PathBuf::new(),
            prefix_args: Vec::new(),
            acp_command: None,
            stream_json_command: None,
            rpc_command: None,
            app_server_command: None,
            authentication: AuthenticationStatus::Unknown,
            origin: ProviderOrigin::UserBinary,
            launch_args: None,
            pickable: Some(false),
            installed_version: None,
            latest_version: crate::registry::cached_latest_npm_version(package),
            install_channel: InstallChannel::Npm,
            npm_package: Some(package),
            tools: mcp_tools_for(spec.id),
            spawn_path_env: None,
            launch_directory: None,
        });
    }
    local
}

#[cfg(feature = "server")]
fn registry_agent(
    directories: &[PathBuf],
    native: &ProviderDiscovery,
    entry: crate::registry::RegistryNpxEntry,
) -> InstalledAgent {
    let crate::registry::RegistryNpxEntry { id, package, args } = entry;
    let pickable = registry_picker_policy(&id, native);
    let (acp_command, npx_directory) = npx_acp_command(directories, &package, &args)
        .map_or((None, PathBuf::new()), |(argv, directory)| {
            (Some(argv), directory)
        });
    // Resolved before the literal moves `id` into the row.
    let tools = mcp_tools_for(&id);
    InstalledAgent {
        id,
        aliases: &[],
        installed: true,
        executable: PathBuf::from(&package),
        prefix_args: Vec::new(),
        acp_command,
        stream_json_command: None,
        rpc_command: None,
        app_server_command: None,
        authentication: AuthenticationStatus::Unknown,
        origin: ProviderOrigin::NpxWrapper,
        launch_args: Some(args),
        pickable,
        installed_version: None,
        latest_version: crate::registry::split_package_version(&package)
            .and_then(crate::provider_catalog::cap_external_version),
        install_channel: InstallChannel::NpxRegistry,
        npm_package: None,
        tools,
        spawn_path_env: None,
        launch_directory: Some(npx_directory).filter(|directory| !directory.as_os_str().is_empty()),
    }
}

#[cfg(feature = "server")]
fn registry_picker_policy(id: &str, native: &ProviderDiscovery) -> Option<bool> {
    let covering_native = REGISTRY_NATIVE_CHAT_COVERAGE
        .iter()
        .find(|(wrapper, _)| id.eq_ignore_ascii_case(wrapper))
        .map(|(_, native_id)| *native_id)?;
    native
        .agents
        .iter()
        .any(|agent| {
            agent.origin == ProviderOrigin::UserBinary
                && agent.id.eq_ignore_ascii_case(covering_native)
                && chat_protocol(agent).is_some()
        })
        .then_some(false)
}

#[cfg(feature = "server")]
fn launch_program_is_cmd_or_bat(program: &Path) -> bool {
    program
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
}

#[cfg(feature = "server")]
fn npx_acp_command(
    directories: &[PathBuf],
    package: &str,
    args: &[String],
) -> Option<(Vec<String>, PathBuf)> {
    let launch = resolve_launch_command_in_paths(directories, "npx")?;
    if launch_program_is_cmd_or_bat(&launch.program) {
        return None;
    }
    let mut extra = Vec::with_capacity(2 + args.len());
    extra.push("-y".to_string());
    extra.push(package.to_string());
    extra.extend(args.iter().cloned());
    let extra_refs: Vec<&str> = extra.iter().map(String::as_str).collect();
    Some((
        protocol_argv(&launch.program, &launch.prefix_args, &extra_refs),
        launch.source_directory,
    ))
}

#[cfg(feature = "server")]
fn merge_native_beats_registry(
    mut local: ProviderDiscovery,
    registry: Vec<InstalledAgent>,
) -> ProviderDiscovery {
    let occupied: HashSet<String> = local
        .agents
        .iter()
        .flat_map(|agent| {
            std::iter::once(agent.id.clone())
                .chain(agent.aliases.iter().map(|alias| (*alias).to_string()))
        })
        .map(|name| name.to_ascii_lowercase())
        .collect();
    for row in registry {
        if occupied.contains(&row.id.to_ascii_lowercase()) {
            continue;
        }
        local.agents.push(row);
    }
    local
}

/// PATH scan plus registry, then the named row if it exists.
#[cfg(feature = "server")]
pub fn find_in_catalog(
    id: &str,
    fetch: &dyn crate::registry::RegistryFetch,
    cache_dir: &Path,
) -> Option<InstalledAgent> {
    #[cfg(windows)]
    {
        find_in_catalog_with_path_source(
            id,
            fetch,
            cache_dir,
            &crate::windows_path_env::RegistryPathSource,
        )
    }
    #[cfg(not(windows))]
    {
        find_in_catalog_in_paths(id, fetch, cache_dir, &discovery_directories())
    }
}

/// [`find_in_catalog`] with the environment behind the seam, mirroring
/// [`find_available_with_path_source`].
#[cfg(all(windows, feature = "server"))]
pub(crate) fn find_in_catalog_with_path_source(
    id: &str,
    fetch: &dyn crate::registry::RegistryFetch,
    cache_dir: &Path,
    source: &dyn crate::windows_path_env::WindowsPathSource,
) -> Option<InstalledAgent> {
    let snapshot = snapshot_of(source);
    let mut agent = find_in_catalog_in_paths(id, fetch, cache_dir, &snapshot.directories())?;
    crate::windows_path_env::attach_spawn_path_env(std::slice::from_mut(&mut agent), &snapshot);
    Some(agent)
}

#[cfg(feature = "server")]
pub(crate) fn find_in_catalog_in_paths(
    id: &str,
    fetch: &dyn crate::registry::RegistryFetch,
    cache_dir: &Path,
    directories: &[PathBuf],
) -> Option<InstalledAgent> {
    discover_catalog_in_paths(fetch, cache_dir, directories)
        .agents
        .into_iter()
        .find(|agent| agent.installed && agent.id == id)
}

pub(crate) fn executable_file_exists(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

fn launch_path_candidates(dir: &Path, command: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        launch_path_candidates_for_pathext(dir, command, std::env::var("PATHEXT").ok().as_deref())
    }

    #[cfg(not(windows))]
    {
        launch_path_candidates_for_pathext(dir, command, None)
    }
}

fn launch_path_candidates_for_pathext(
    dir: &Path,
    command: &str,
    pathext: Option<&str>,
) -> Vec<PathBuf> {
    let base = dir.join(command);

    #[cfg(not(windows))]
    {
        let _ = pathext;
        vec![base]
    }

    #[cfg(windows)]
    {
        if let Some(extension) = Path::new(command).extension().and_then(|ext| ext.to_str()) {
            return is_direct_launch_extension(&format!(".{extension}"))
                .then_some(base)
                .into_iter()
                .collect();
        }

        let pathext = pathext
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(DEFAULT_PATHEXT);
        pathext
            .split(';')
            .map(str::trim)
            .filter(|extension| !extension.is_empty())
            .filter_map(|extension| {
                let extension = if extension.starts_with('.') {
                    extension.to_string()
                } else {
                    format!(".{extension}")
                };
                is_direct_launch_extension(&extension)
                    .then(|| dir.join(format!("{command}{extension}")))
            })
            .collect()
    }
}

#[cfg(windows)]
// PATHEXT also names interpreter scripts such as PS1, VBS, and JS. They are
// intentionally excluded: spawn_process uses Command/CreateProcess directly,
// without an interpreter, so a PS1-only installation is not launchable.
fn is_direct_launch_extension(extension: &str) -> bool {
    [".COM", ".EXE", ".BAT", ".CMD"]
        .iter()
        .any(|known| known.eq_ignore_ascii_case(extension))
}

fn resolve_direct_program(paths: &[PathBuf], command: &str) -> Option<(PathBuf, PathBuf)> {
    paths.iter().find_map(|dir| {
        launch_path_candidates(dir, command)
            .into_iter()
            .find_map(|path| {
                executable_file_exists(&path)
                    .then(|| absolute_path(&path).map(|program| (program, dir.clone())))
            })
            .flatten()
    })
}

fn resolve_launch_command_in_paths(paths: &[PathBuf], command: &str) -> Option<ResolvedLaunch> {
    let (program, source_directory) = resolve_direct_program(paths, command)?;
    #[cfg(windows)]
    if let Some(unwrapped) = unwrap_windows_npm_cmd_shim(&program, paths) {
        return Some(unwrapped);
    }
    Some(ResolvedLaunch::launched(
        program,
        Vec::new(),
        source_directory,
    ))
}

/// Resolve npm through the same direct PATH/shim path used for provider
/// commands. The returned pair is CreateProcess's program and argv prefix;
/// no shell receives the update command.
#[cfg(feature = "server")]
pub(crate) fn resolve_npm_command(paths: &[PathBuf]) -> Option<(PathBuf, Vec<String>)> {
    resolve_launch_command_in_paths(paths, "npm").map(|launch| (launch.program, launch.prefix_args))
}

/// npm's `cmd-shim` writes a batch that assigns `_prog` to `node` (or a
/// sibling `node.exe`) and then launches `"%_prog%" "%dp0%\<script>" %*`.
/// Measured on this machine for `codex.cmd`, `pi.cmd`, and `qwen.cmd`.
fn is_npm_node_cmd_shim(contents: &str) -> bool {
    let lower = contents.to_ascii_lowercase();
    lower.contains(r#"set "_prog=node""#) || lower.contains(r#"set "_prog=%dp0%\node.exe""#)
}

/// Relative script path from an npm cmd-shim, if the launch line is the
/// measured `"%_prog%" "%dp0%\<script>" %*` form. Anything else is left alone.
fn npm_cmd_shim_script_relative(contents: &str) -> Option<&str> {
    if !is_npm_node_cmd_shim(contents) {
        return None;
    }

    let mut rest = contents;
    while let Some(offset) = rest.find(r#""%_prog%""#) {
        rest = &rest[offset + r#""%_prog%""#.len()..];
        let trimmed = rest.trim_start_matches([' ', '\t']);
        let Some((relative, after)) = split_quoted_dp0_path(trimmed) else {
            continue;
        };
        let after = after.trim_start_matches([' ', '\t']);
        if after.starts_with("%*") && !relative.is_empty() {
            return Some(relative);
        }
    }
    None
}

fn split_quoted_dp0_path(input: &str) -> Option<(&str, &str)> {
    let rest = input.strip_prefix('"')?;
    let rest = rest
        .strip_prefix("%dp0%")
        .or_else(|| rest.strip_prefix("%~dp0"))?;
    let rest = rest.trim_start_matches(['\\', '/']);
    let end = rest.find('"')?;
    Some((&rest[..end], &rest[end + 1..]))
}

/// npm's own launcher shim (npx.cmd, npm.cmd) uses a different batch shape
/// than per-package cmd-shims. It assigns a node executable variable and a
/// script variable from `%~dp0`, then invokes
/// `"%NODE_VAR%" "%SCRIPT_VAR%" %*`. Measured on this machine for npx.cmd
/// (npm 10.x).
///
/// Recognized by the `SET "NODE_EXE=%~dp0\node.exe"` (or `%dp0%`) line.
fn is_npm_launcher_shim(contents: &str) -> bool {
    let lower = contents.to_ascii_lowercase();
    lower.contains(r#"set "node_exe=%~dp0\node.exe""#)
        || lower.contains(r#"set "node_exe=%dp0%\node.exe""#)
        || lower.contains(r#"set "node_exe=%~dp0/node.exe""#)
        || lower.contains(r#"set "node_exe=%dp0%/node.exe""#)
}

/// Relative script path from an npm launcher shim, if recognized.
///
/// Parses `SET "VAR=%~dp0\..."` assignments to find the node executable
/// variable (value ends in `node.exe`) and the script variable (value ends in
/// `.js`). Then looks for the final invocation line
/// `"%NODE_VAR%" "%SCRIPT_VAR%" %*` and returns the relative path from the
/// last static `%~dp0` / `%dp0%` assignment for the script variable.
///
/// The FOR /F dynamic override (if present) is intentionally ignored — the
/// static `%~dp0\node_modules\npm\bin\npx-cli.js` always exists in a real npm
/// install and is the correct fallback.
fn npm_launcher_shim_script_relative(contents: &str) -> Option<&str> {
    if !is_npm_launcher_shim(contents) {
        return None;
    }

    let mut node_var_name: Option<&str> = None;
    let mut script_var_name: Option<&str> = None;
    let mut script_relative: Option<&str> = None;

    for line in contents.lines() {
        let trimmed = line.trim();
        // Match: SET "VAR=%~dp0\path" or SET "VAR=%dp0%\path". A non-SET line
        // (@ECHO, IF, FOR, the final invocation) is skipped, not a parse
        // failure — the launcher body is mostly non-SET lines.
        let Some(rest) = trimmed
            .strip_prefix("SET \"")
            .or_else(|| trimmed.strip_prefix("set \""))
        else {
            continue;
        };
        let Some(eq) = rest.find('=') else {
            continue;
        };
        let var_name = &rest[..eq];
        let value = &rest[eq + 1..];
        let value = value.strip_suffix('"').unwrap_or(value);

        // Not a dp0-relative assignment (e.g. SET "NODE_EXE=node") — skip.
        let Some(relative) = value
            .strip_prefix("%~dp0\\")
            .or_else(|| value.strip_prefix("%dp0%\\"))
            .or_else(|| value.strip_prefix("%~dp0/"))
            .or_else(|| value.strip_prefix("%dp0%/"))
        else {
            continue;
        };

        let value_lower = value.to_ascii_lowercase();
        if value_lower.ends_with("node.exe") {
            node_var_name = Some(var_name);
        } else if value_lower.ends_with(".js") {
            script_var_name = Some(var_name);
            script_relative = Some(relative);
        }
    }

    let node_var = node_var_name?;
    let script_var = script_var_name?;
    let relative = script_relative?;

    // Find the final invocation line: "%NODE_VAR%" "%SCRIPT_VAR%" %*
    let node_pat = format!("\"%{node_var}%\"");
    let script_pat = format!("\"%{script_var}%\"");

    let mut rest = contents;
    while let Some(offset) = rest.find(&node_pat) {
        let after_node = &rest[offset + node_pat.len()..];
        let after_node = after_node.trim_start_matches([' ', '\t']);
        let Some(after_script) = after_node.strip_prefix(&script_pat) else {
            rest = &rest[offset + node_pat.len()..];
            continue;
        };
        let after_script = after_script.trim_start_matches([' ', '\t']);
        if after_script.starts_with("%*") && !relative.is_empty() {
            return Some(relative);
        }
        rest = &rest[offset + node_pat.len()..];
    }
    None
}

#[cfg(windows)]
fn unwrap_windows_npm_cmd_shim(shim: &Path, search_paths: &[PathBuf]) -> Option<ResolvedLaunch> {
    let extension = shim.extension()?.to_str()?;
    if !extension.eq_ignore_ascii_case("cmd") && !extension.eq_ignore_ascii_case("bat") {
        return None;
    }

    let contents = std::fs::read_to_string(shim).ok()?;

    // Try per-package cmd-shim shape first ("%_prog%" "%dp0%\<script>" %*).
    if let Some(relative) = npm_cmd_shim_script_relative(&contents) {
        return resolve_shim_script(shim, search_paths, relative);
    }

    // Fall back to npm launcher shape ("%NODE_EXE%" "%NPX_CLI_JS%" %*).
    if let Some(relative) = npm_launcher_shim_script_relative(&contents) {
        return resolve_shim_script(shim, search_paths, relative);
    }

    None
}

/// Shared resolution logic: build the script path from the shim directory +
/// relative path, check `..` traversal and existence, resolve `node.exe` from
/// the shim sibling or PATH, and return a `ResolvedLaunch`.
#[cfg(windows)]
fn resolve_shim_script(
    shim: &Path,
    search_paths: &[PathBuf],
    relative: &str,
) -> Option<ResolvedLaunch> {
    let shim_dir = shim.parent()?;
    let mut script = shim_dir.to_path_buf();
    for component in relative.split(['/', '\\']) {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            return None;
        }
        script.push(component);
    }
    if !script.is_file() {
        return None;
    }

    let node = {
        let local_node = shim_dir.join("node.exe");
        if executable_file_exists(&local_node) {
            absolute_path(&local_node)
        } else {
            resolve_direct_program(search_paths, "node").map(|(program, _)| program)
        }
    }?;
    let script = absolute_path(&script)?;
    Some(ResolvedLaunch::launched(
        node,
        vec![script.to_string_lossy().into_owned()],
        shim_dir.to_path_buf(),
    ))
}

fn absolute_path(path: &Path) -> Option<PathBuf> {
    let absolute = std::fs::canonicalize(path).ok().or_else(|| {
        if path.is_absolute() {
            Some(path.to_path_buf())
        } else {
            std::env::current_dir().ok().map(|cwd| cwd.join(path))
        }
    })?;

    #[cfg(windows)]
    {
        Some(normalize_windows_path(absolute))
    }

    #[cfg(not(windows))]
    {
        Some(absolute)
    }
}

#[cfg(windows)]
fn normalize_windows_path(path: PathBuf) -> PathBuf {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let strip_prefix = |prefix: &str| {
        let path = path.as_os_str().encode_wide().collect::<Vec<_>>();
        let prefix = OsStr::new(prefix).encode_wide().collect::<Vec<_>>();
        path.starts_with(&prefix)
            .then(|| OsString::from_wide(&path[prefix.len()..]))
    };

    if let Some(rest) = strip_prefix(r"\\?\UNC\") {
        let mut normalized = OsString::from_wide(&[b'\\' as u16, b'\\' as u16]);
        normalized.push(rest);
        return PathBuf::from(normalized);
    }

    if let Some(rest) = strip_prefix(r"\\?\") {
        let rest = PathBuf::from(rest);
        if rest.is_absolute() {
            return rest;
        }
    }

    path
}

#[cfg(test)]
fn probe_version(agent: &InstalledAgent) -> std::io::Result<Output> {
    Command::new(&agent.executable)
        .args(&agent.prefix_args)
        .arg("--version")
        .output()
}

#[cfg(test)]
#[path = "provider_catalog_tests.rs"]
mod tests;
