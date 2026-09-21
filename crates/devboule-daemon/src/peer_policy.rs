//! Peer policy: what a paired connection may do, in one closed place.
//!
//! The decision is a **closed match with no `_` arm** over every
//! `ClientMessage` variant. Adding a variant without deciding its peer policy
//! is a compile error, which is the exhaustiveness guarantee the design asks
//! for (`DESIGN-remote-agents.md` §8 R2, §8b A1/A12). A runtime
//! `ALL_SAMPLES` iteration would only restate what the compiler already
//! enforces.
//!
//! The 1a surface was deliberately tiny; slice 3 opens exactly five session
//! variants, each under the capability that names the act (`view`, `send`,
//! `answer_permissions`, `create_sessions` — §8b A9/A11/A12). `Status`,
//! pairing, capability changes and the tool bridge stay refused to a peer
//! whatever it holds, because no capability names those acts. Scope — *which*
//! sessions an allowed request reaches — is not decided here: it is the owner
//! projection in `server.rs` plus the origin branch of `check_user_owner`.

use devboule_protocol::{ClientMessage, SessionKind, SessionOrigin};

/// The role a peer was paired as. Stored in the `peers` row; the transcript
/// separates the two Noise handshakes, so the role cannot be changed by the
/// other side. Defined once, in the protocol crate: the wire and this policy
/// must not be able to disagree about the spelling.
pub use devboule_protocol::PeerRole;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerDecision {
    Allow,
    /// A capability label, not prose: it becomes the `CapabilityNotSupported`
    /// payload so the peer can tell what was refused.
    Deny(&'static str),
}

/// The capability names this gate reads, spelled once. `PEER_CAPS` in the
/// protocol crate is the wire set a `PeerSetCaps` may name; the test below
/// pins these five to it so a rename cannot leave the gate enforcing a
/// capability nobody can hold.
pub const CAP_VIEW: &str = "view";
/// `send` names two target scopes: `SessionSend` keeps the peer's existing
/// own-origin scope, while `AgentMessageSend` lets a daemon-role peer write
/// only into local sessions of the user who paired that device. The frame's
/// target rule enforces the latter; this capability still gates the act.
pub const CAP_SEND: &str = "send";
pub const CAP_ANSWER_PERMISSIONS: &str = "answer_permissions";
pub const CAP_CREATE_SESSIONS: &str = "create_sessions";
/// The peer roster (`PeerAgentsList`) has a capability of its own, and it is
/// deliberately **absent from `PEER_DEFAULT_CAPS`**: `view` is one every
/// pairing already holds, and reading the pairing user's whole live roster is
/// a disclosure no pairing should carry silently. Nothing changes for an
/// existing device until a person grants this per device.
pub const CAP_ROSTER: &str = "roster";

/// The audit outcome for a request refused because it would run a session
/// without asking the user's permission (`DESIGN-remote-agents.md` §8b A5).
/// One spelling, used by the refusal and by the audit row it writes.
pub const PROMPT_SKIPPING_REFUSED: &str = "prompt_skipping_refused";

/// The audit outcome for a request refused because it reached for an ACP
/// session mode (`DESIGN-remote-agents.md` §8b A5, §8 R3): ACP mode ids are
/// defined by the agent at run time, so there is no list to vet them against
/// and a paired device may not choose one at all.
pub const ACP_MODES_UNVETTED_REFUSED: &str = "acp_modes_unvetted_refused";

/// What a peer is told when it names an ACP mode. One spelling: the gate
/// (`server.rs`) and the sessions layer (`session.rs`) both refuse it.
pub const ACP_MODES_UNVETTED_MESSAGE: &str =
    "ACP session modes are defined by the agent, so a paired device cannot choose one.";

/// Why a send from a paired device that carries attachments is refused, for
/// now (`server.rs::peer_refusal_before_mode`). Temporary: it goes away when
/// the deposit branch's counter lands and `budget_for` has a caller.
pub const PEER_ATTACHMENTS_UNSUPPORTED: &str =
    "attachments from a paired device are not accepted yet";

/// Rendered pages one composer turn may carry (the app's own cap).
const BUDGET_PAGES_PER_TURN: u64 = 200;
/// What one rendered PDF page weighs once stored.
const BUDGET_BYTES_PER_PAGE: u64 = 96 * 1024;
/// One frame of inline attachments, on top of the pages.
const BUDGET_INLINE_FRAME_BYTES: u64 = 384 * 1024;

/// The attachment budget of one origin, in stored bytes.
///
/// The peer case is the **local derivation reused**, not a second invention:
/// 200 rendered PDF pages at 96 KiB plus one frame of inline attachments, the
/// number the brief calls 20 MiB. The peer case is unmeasured, and it must be
/// re-derived the first time a paired device actually sends something rather
/// than inherited: a phone's working set is not a desktop's.
///
/// The counter itself belongs to the peer's deposit branch, keyed on
/// `OwnerId::user` and walked through the session registry. This function is
/// where that counter will read its figure, which is why it takes the origin.
#[allow(dead_code)] // the peer's deposit branch is the caller to come.
pub(crate) fn budget_for(origin: &SessionOrigin) -> u64 {
    let _ = origin;
    BUDGET_PAGES_PER_TURN * BUDGET_BYTES_PER_PAGE + BUDGET_INLINE_FRAME_BYTES
}

/// May `role`, holding `caps`, send `request`?
///
/// `caps` is the peer's own capability set, read from its `peers` row. The
/// four names are the whole permission model for a paired device (§8b A9/A11):
/// a variant that no capability names is refused to every peer, and a variant
/// that one names is allowed exactly when the peer holds it.
///
/// `role` does not decide permission here: the capability set does. It stays
/// in the signature because it decides *scope* one layer down (the owner
/// projection in `server.rs` and the origin branch of `check_user_owner`), and
/// because a future role-specific rule gets one place to live rather than a
/// second allowlist.
pub fn peer_allows(role: PeerRole, caps: &[String], request: &ClientMessage) -> PeerDecision {
    let _ = role;
    match request {
        // The handshake itself and the read-only liveness/identity pair.
        // `Hello` never reaches `dispatch()` — the connection loop answers a
        // second hello before the gate — but the arm is here so the match
        // stays exhaustive and every decision stays visible in one place.
        ClientMessage::Hello(_) => PeerDecision::Allow,
        ClientMessage::Ping { .. } => PeerDecision::Allow,
        // The two list acts are reads, and every read on this surface is
        // `view` (§8b A11): `SessionsList` and `DevicesList` are how a paired
        // device sees anything at all, so a peer stripped of `view` — only a
        // `Daemon` peer can be, since `validate_caps` will not remove it from a
        // `Client` — reaches neither. Both were unconditional `Allow` before
        // the slice-3 fix pass, which made "no capability" a capability.
        ClientMessage::SessionsList { .. } => with_capability(caps, CAP_VIEW),
        // Role-projected at the dispatch site; a `Daemon` peer sees only
        // `{device_id, display_name, role, online}` (design §8b A13).
        ClientMessage::DevicesList { .. } => with_capability(caps, CAP_VIEW),
        // The live agent roster of the responding device, scoped on the
        // responder to the user that approved the pairing (`paired_by_user`,
        // a fact the responding daemon wrote itself). Reading a roster is
        // observation, but it is a disclosure `view` does not cover — every
        // pairing holds `view`, and the roster is the pairing user's whole
        // live surface — so it rides its own capability, absent from
        // `PEER_DEFAULT_CAPS` and granted per device. Who answers is not
        // decided in this arm; the capability set alone decides.
        ClientMessage::PeerAgentsList { .. } => with_capability(caps, CAP_ROSTER),

        // Slice 3: the five session variants a paired device may reach, each
        // under the capability that names the act. `view` is what makes a peer
        // a viewer at all; it is the one capability `validate_caps` will not
        // remove from a `Client` (A11). `SessionSend` keeps the peer's own
        // origin scope; `AgentMessageSend` uses the pairing user's local
        // target scope, because its sender may live on the far device.
        ClientMessage::SessionAttach { .. } => with_capability(caps, CAP_VIEW),
        ClientMessage::SessionCreate { .. } => with_capability(caps, CAP_CREATE_SESSIONS),
        ClientMessage::SessionSend { .. } => with_capability(caps, CAP_SEND),
        ClientMessage::AgentMessageSend { .. } => with_capability(caps, CAP_SEND),
        // A deposit is meaningless except as the precursor to a send, so it
        // holds no opinion of its own: the same predicate, deliberately. A
        // peer allowed to send must be able to deposit or it can never attach
        // a picture, which is most of the point of a paired phone; and a peer
        // not allowed to send must not be able to deposit, or it writes bytes
        // into a session folder that nothing on this machine can consume.
        ClientMessage::SessionDeposit { .. } => with_capability(caps, CAP_SEND),
        ClientMessage::SessionPermissionRespond { .. } => {
            with_capability(caps, CAP_ANSWER_PERMISSIONS)
        }
        // Switching the mode changes what the session will do with the next
        // turn, which is the act `send` names. The prompt-skipping list (§8b
        // A5) refuses the specific modes on top of this, in `dispatch`.
        ClientMessage::SessionSetMode { .. } => with_capability(caps, CAP_SEND),

        // Pairing and revocation are local acts. A peer that could start a
        // pairing or revoke another peer would be able to change this device's
        // trusted set, which is exactly what pairing exists to prevent. No
        // capability names them, so no capability opens them.
        ClientMessage::PairingStart { .. } => PeerDecision::Deny("pairing.start"),
        ClientMessage::PairingComplete { .. } => PeerDecision::Deny("pairing.complete"),
        ClientMessage::PairingConfirm { .. } => PeerDecision::Deny("pairing.confirm"),
        ClientMessage::PeerRevoke { .. } => PeerDecision::Deny("peer.revoke"),
        ClientMessage::PeerSetCaps { .. } => PeerDecision::Deny("peer.set_caps"),
        // A door that hands over deposited bytes must not become a way for a
        // paired device to read them: every content read on this surface is
        // refused, while only list reads ride on `view`.
        ClientMessage::SessionAttachmentRead { .. } => PeerDecision::Deny("attachment.read"),

        // Tool policies are this device's own settings, read and written by
        // its user through the app. A paired device toggling them would be
        // changing what this machine hands to its agents, so both the read
        // and the write are refused rather than projected.
        ClientMessage::ToolPolicyGet { .. } => PeerDecision::Deny("tool.policy.get"),
        ClientMessage::ToolPolicySet { .. } => PeerDecision::Deny("tool.policy.set"),

        // The agent profiles are the same kind of object as the tool gates and
        // are refused for the same reason, both halves: a profile carries the
        // mode, the model and the tool overlay this machine's agents are created
        // in, and the standing instructions are text that goes into every
        // created agent's prompt. A paired device that could set them would be
        // writing what this machine's agents do — and a device that could read
        // them would be reading rules it was never given.
        ClientMessage::AgentProfilesGet { .. } => PeerDecision::Deny("agent.profiles.get"),
        ClientMessage::AgentProfilesSet { .. } => PeerDecision::Deny("agent.profiles.set"),

        // The delegation switch is this device's own authority setting: it
        // decides whether an agent on this machine may answer its child's
        // permission cards, so a paired device that could set it would be
        // granting itself answers this machine's human never gave, and one
        // that could read it would learn whether the door is open. Both
        // halves refused, the same way the profile store is — whichever
        // capability the peer holds.
        ClientMessage::DelegationGet { .. } => PeerDecision::Deny("delegation.get"),
        ClientMessage::DelegationSet { .. } => PeerDecision::Deny("delegation.set"),

        // The provider vocabulary is the profile store's companion read: it
        // says what this machine's providers offer, which is the other half of
        // what a profile stores. A paired device that could read it would be
        // reading this machine's capability shape, and the handshake
        // capability that advertises the query to the app is deliberately not
        // a peer capability — no peer may hold it, so no arm but this one.
        ClientMessage::ProviderVocabularyGet { .. } => {
            PeerDecision::Deny("provider.vocabulary.get")
        }

        // Local-only information: pid, instance id, live counts and the
        // secret-store selector, none of which a peer needs. Peers use `Ping`
        // and `DevicesList.self_info` (muse M7, §8b A13).
        ClientMessage::Status { .. } => PeerDecision::Deny("status"),
        ClientMessage::DaemonDiagnostics { .. } => PeerDecision::Deny("diagnostics"),

        // The global deny list of §8 R1: destructive or configuration
        // changes, never reachable from a paired device.
        ClientMessage::Shutdown { .. } => PeerDecision::Deny("shutdown"),
        ClientMessage::SessionDelete { .. } => PeerDecision::Deny("session.delete"),
        ClientMessage::JournalRetentionSet { .. } => PeerDecision::Deny("journal.retention.set"),
        ClientMessage::ProvidersRefresh { .. } => PeerDecision::Deny("providers.refresh"),
        ClientMessage::ProviderUpdate { .. } => PeerDecision::Deny("provider.update"),
        ClientMessage::ProjectAdd { .. } => PeerDecision::Deny("project.add"),
        ClientMessage::WorkspaceCreate { .. } => PeerDecision::Deny("workspace.create"),
        ClientMessage::WorkspaceDelete { .. } => PeerDecision::Deny("workspace.delete"),
        ClientMessage::Invoke { .. } => PeerDecision::Deny("invoke"),
        ClientMessage::SessionClaim { .. } => PeerDecision::Deny("session.claim"),
        ClientMessage::SessionResume { .. } => PeerDecision::Deny("session.resume"),
        ClientMessage::SessionReportAgent { .. } => PeerDecision::Deny("session.report_agent"),

        // Still outside a peer's reach after slice 3: their own slices or
        // never. Refused rather than half-supported.
        ClientMessage::JournalUsage { .. } => PeerDecision::Deny("journal.usage"),
        ClientMessage::JournalRetentionGet { .. } => PeerDecision::Deny("journal.retention.get"),
        ClientMessage::SessionDetach { .. } => PeerDecision::Deny("session.detach"),
        ClientMessage::SessionClose { .. } => PeerDecision::Deny("session.close"),
        ClientMessage::SessionStop { .. } => PeerDecision::Deny("session.stop"),
        ClientMessage::SessionResize { .. } => PeerDecision::Deny("session.resize"),
        ClientMessage::SessionInterrupt { .. } => PeerDecision::Deny("session.interrupt"),
        ClientMessage::SessionSetModel { .. } => PeerDecision::Deny("session.set_model"),
        ClientMessage::SessionsWatch { .. } => PeerDecision::Deny("sessions.watch"),
        ClientMessage::SessionsUnwatch { .. } => PeerDecision::Deny("sessions.unwatch"),
        ClientMessage::SessionsPresence { .. } => PeerDecision::Deny("sessions.presence"),
        ClientMessage::ProjectsList { .. } => PeerDecision::Deny("projects.list"),
        ClientMessage::WorkspacesList { .. } => PeerDecision::Deny("workspaces.list"),
        ClientMessage::ProvidersList { .. } => PeerDecision::Deny("providers.list"),
    }
}

/// Allow when `caps` holds `capability`; otherwise refuse, naming the missing
/// capability so the peer's error says what it would need.
fn with_capability(caps: &[String], capability: &'static str) -> PeerDecision {
    if caps.iter().any(|cap| cap == capability) {
        PeerDecision::Allow
    } else {
        PeerDecision::Deny(capability)
    }
}

/// Whether `mode_id` is a mode that can run without asking the target
/// device's user (`DESIGN-remote-agents.md` §8b A5).
///
/// Called from the peer gate twice: the audit path (`server.rs::peer_outcome`)
/// labels a denial that asked for one of these modes `prompt_skipping_refused`,
/// and slice 3's refusal (`peer_gate::peer_mode_refusal_for_conn`) refuses the request
/// outright — a paired device never drives a session that will not ask this
/// machine's user.
///
/// The lists are concrete because "prompt skipping" is per provider and is
/// not exposed uniformly. Two consequences for the caller:
///
/// - Codex `auto` is **not** here: it still prompts, so it is allowed.
/// - ACP modes are defined by the agent at runtime. This function cannot
///   answer for them and returns `false`; A5's ACP rule is the separate
///   "only the agent's ask/default mode" allowlist, which the caller must
///   apply with the agent's own mode list. Do not use this function alone to
///   decide whether a remote-origin ACP session may be driven.
pub fn prompt_skipping_mode(kind: SessionKind, mode_id: &str) -> bool {
    // The lists are per-family facts and live in the provider impls (pass
    // 2b); this shim is the same signature the peer gate and the walking
    // tests have always read, now answered through the registry.
    crate::session::catalog_registry()
        .provider_for_kind(&kind)
        .prompt_skipping_mode(mode_id)
}

/// The `unattended` marker for one session: the honest answer to "can this
/// session pass a permission moment with no human answering", derived from
/// the mode the daemon **delivered** (`DESIGN-what-unattended-means.md`).
///
/// This is the sibling of [`prompt_skipping_mode`] — in the same home, keyed
/// the same way, and **never merged with it**: a mode can skip prompts for a
/// peer and still be un-establishable for this marker. The answers are
/// per-family facts and live in the provider impls (pass 2d); this shim is
/// the same signature the birth marker, the child road, the profile
/// prediction and the tests have always read, now answered through the
/// registry. The rule, unchanged, in the shape the impls carry it:
///
/// - **Route A — the daemon answers itself.** A delivered mode carrying one
///   of the provider-agnostic ids `provider_catalog::mode_is_auto_answered`
///   lists is answered by the daemon's own broker, whatever *agent* family
///   the session belongs to; the shared helper in `provider.rs` is the one
///   list, and every agent impl's dictionary sits behind it. A terminal is
///   **not** an agent family: the broker's auto-answer call sites cover the
///   agent clients only, and a terminal has no permission mechanism at all,
///   so there is no permission moment for anything to answer and no mode id
///   — including a route-A id — can make one exist. The Terminal impl, not
///   this list, decides a terminal, and it says `no` (audit R2b-1 §3.1: a
///   terminal created with `mode: "bypass"` used to answer `yes` because
///   the old check ran first).
/// - **Route B — the daemon authored the knob.** For Claude, Codex and Pi
///   the dictionary is the client family's own mode table
///   (`claude_view::unattended_answer`, `codex_view::unattended_answer`,
///   `pi_client::unattended_answer`) — the vocabulary the daemon delivers
///   and therefore knows. A mode id those tables do not carry is a mode the
///   daemon never authored, and the answer is `unknown`, never `no`: an
///   unauthored id is an absence of knowledge, and reading it as "a human is
///   watching" is the lie in its most dangerous direction.
/// - **Cannot establish.** An ACP agent's modes are `{id, name, description}`
///   prose the agent authored; no table here judges them. Outside the three
///   route-A ids the answer is `unknown`. The same answer covers what nobody
///   said: an absent or empty delivered mode is `unknown` for a family the
///   daemon does not set a mode for, and a terminal — which has no
///   permission mechanism at all, and never had one to be told about —
///   carries `no`, exactly what the collapsed `bool` this marker replaces
///   recorded for it. A user-defined provider's child from a config file
///   lands on the ACP answer because `session_kind_for(provider)` — the one
///   provider-name match on this path, read at the birth before this
///   function is consulted — maps every name that is not one of the three
///   authored families to [`SessionKind::Acp`], whose impl is `unknown`: no
///   name beyond the three is ever *interpreted*, and an unrecognised one
///   fails toward the honest answer, not toward `no`.
pub fn unattended_mode(
    kind: SessionKind,
    delivered_mode: Option<&str>,
) -> devboule_protocol::UnattendedState {
    // The per-family answers moved into the impls; this keeps the signature
    // the callers have always read.
    crate::session::catalog_registry()
        .provider_for_kind(&kind)
        .unattended_mode(delivered_mode)
        .into()
}

/// §8b A5/R3, the whole rule: why a paired device may not choose `mode_id` for
/// a `kind` session, or `None` when it may.
///
/// For Claude, Codex, Pi and Terminal the answer is the concrete list above.
/// For ACP it is *every* mode id, including the ones that look like `ask` or
/// `default`: the agent defines its own modes at run time, this daemon has no
/// list to vet them against, and an id that means "ask the user" to one agent
/// can mean "run unattended" to the next. Refusing every id is the only
/// fail-closed answer, and it is the conservative form of A5's ACP sentence
/// ("created only in the mode the agent marks as its ask/default"): a remote
/// ACP create carries no mode at all, so the agent's own default stands, and a
/// remote `SessionSetMode` never lands.
///
/// The reasons are the two audit labels, so the trail says which rule fired.
pub fn mode_refusal(kind: SessionKind, mode_id: &str) -> Option<&'static str> {
    let provider = crate::session::catalog_registry().provider_for_kind(&kind);
    if provider.modes_unvetted() {
        return Some(ACP_MODES_UNVETTED_REFUSED);
    }
    provider
        .prompt_skipping_mode(mode_id)
        .then_some(PROMPT_SKIPPING_REFUSED)
}

/// What one MCP broker tool performs, in the wire vocabulary this policy judges.
///
/// `Judged(requests)` — the tool performs these wire acts on the caller's behalf,
/// and the door judges each with [`peer_allows`] (first `Deny` wins). The request
/// fields are placeholders: no `peer_allows` arm reads a field, only the variant
/// and the capability set, so the decision cannot depend on them.
///
/// `Unjudged(reason)` — the tool performs nothing the policy judges, and the
/// reason says why. The only such tool is the ticked-profile list (below).
///
/// `None` (from [`mcp_tool_wire`]) is an unknown tool name — not served by the
/// broker. The door lets it through to the broker's own `Unknown tool` arm,
/// which touches nothing; the closed-table test fails for any *served* name
/// without an arm here, so adding a tool without deciding its row is a red test.
#[derive(Debug)]
#[allow(dead_code)] // `Unjudged.0` is read by the closed-table test, not by prod code.
pub enum McpToolWire {
    Judged(Vec<ClientMessage>),
    Unjudged(&'static str),
}

/// The permission table for the MCP tool door: every served tool's wire
/// equivalent, in one closed place beside the policy it reuses.
///
/// - Roster (`devboule_list_agents`) reads the owner's live agents: the wire
///   read `SessionsList`, the act `view` names.
/// - Devices (`devboule_list_devices`) reads this daemon's paired rows, the
///   wire read `DevicesList`, the same act `view` names.
/// - Peer agents (`devboule_list_peer_agents`) dials one named device for
///   its live roster, the wire read `PeerAgentsList`, judged under its own
///   capability `roster`, never `view`: every pairing holds `view`, and the
///   roster is the pairing user's whole live surface. One call, one dial —
///   never a fan-out.
/// - Activity (`devboule_agent_activity`) reads one of those agents: the same
///   wire read, the same act. Kinds and timestamps only, never transcript.
/// - Profile list (`devboule_list_profiles`) is `Unjudged`: it serves only the
///   ticked subset (name, note, provider, model, mode, unattended prediction)
///   the human enabled for agents to consume — not the full document
///   `AgentProfilesGet` returns (unticked profiles, stable ids, tool overlays,
///   standing instructions), which stays denied to every peer and no tool serves.
///   The tick is the human's authorisation to disclose to agents, including a
///   peer's agents that may create; without the list `create_agent` is
///   undiscoverable (its own error sends the caller to the list).
/// - Send (`devboule_send_message`) puts text into a session: `AgentMessageSend`,
///   the act `send` names. `SessionSend` keeps the peer's own-origin target
///   scope; a daemon-role `AgentMessageSend` instead reaches only local
///   sessions belonging to the user who paired that device.
/// - Create (`devboule_create_agent`) makes a session on the caller's device
///   **and** sends the mandatory `initialPrompt` as that child's first turn
///   (`session.rs::create_session_for_agent`, the same send path
///   `agent_message_send` serves): `SessionCreate` plus `AgentMessageSend`,
///   always. The prompt is not optional — creating through this tool always
///   sends — so a device with `create_sessions` but without `send` is refused
///   the whole tool (first `Deny` wins): *you may create agents, you may not
///   talk to them* means no agent through this tool at all. The placeholder
///   kind never decides: that arm reads only the capability set.
/// - Answer (`devboule_answer_permission`) answers a permission moment:
///   `SessionPermissionRespond`.
/// - Move (`devboule_set_agent_profile`) applies a profile, which declares a
///   mode *and* a model: `SessionSetMode` plus `SessionSetModel`. Both, always —
///   the model-skip when the child already runs the profile's model is a
///   runtime optimisation, not a permission fact, and permission must not depend
///   on it (tomorrow's profile edit would silently widen a peer's grant).
///   Since the model half is denied to every peer, a peer never applies a profile;
///   a peer with `send` may still change modes over the wire `SessionSetMode`.
pub fn mcp_tool_wire(tool: &str) -> Option<McpToolWire> {
    use crate::provider_catalog::{
        MCP_ACTIVITY_TOOL, MCP_ANSWER_PERMISSION_TOOL, MCP_CLOSE_AGENT_TOOL, MCP_CREATE_AGENT_TOOL,
        MCP_IMPORTERS_TOOL, MCP_IMPORTS_TOOL, MCP_LIST_DEVICES_TOOL, MCP_LIST_PEER_AGENTS_TOOL,
        MCP_LIST_PROFILES_TOOL, MCP_NEIGHBORHOOD_TOOL, MCP_ROSTER_TOOL, MCP_SEND_MESSAGE_TOOL,
        MCP_SET_AGENT_PROFILE_TOOL, MCP_STOP_AGENT_TOOL,
    };
    if tool == MCP_ROSTER_TOOL {
        Some(McpToolWire::Judged(vec![ClientMessage::SessionsList {
            id: 0,
        }]))
    } else if tool == MCP_LIST_DEVICES_TOOL {
        // The paired-device discovery read: judged as the wire's `DevicesList`
        // read under `view`, and safe to so judge because the body returns
        // less than either wire shape — four naming fields scoped to the
        // calling session's own user, never the key, the address, the
        // binding, or the pairing user a `Client` peer's whole row carries.
        // The scope comes from the body, never from an argument, so the
        // capability set alone decides here.
        Some(McpToolWire::Judged(vec![ClientMessage::DevicesList {
            id: 0,
        }]))
    } else if tool == MCP_LIST_PEER_AGENTS_TOOL {
        // The one-dial roster read: the wire act is `PeerAgentsList`, judged
        // under its own capability `roster` — every pairing already holds
        // `view`, so `view` cannot be the word that guards the pairing
        // user's whole live surface. One call names one device and makes
        // one dial; whose roster answers is the responder's own pairing-user
        // decision, never a caller argument.
        Some(McpToolWire::Judged(vec![ClientMessage::PeerAgentsList {
            id: 0,
        }]))
    } else if tool == MCP_ACTIVITY_TOOL {
        // A read like the roster: the owner's live agents, no transcript.
        Some(McpToolWire::Judged(vec![ClientMessage::SessionsList {
            id: 0,
        }]))
    } else if tool == MCP_LIST_PROFILES_TOOL {
        Some(McpToolWire::Unjudged(
            "the ticked subset the human enabled for agents; the full-document read stays denied and unserved",
        ))
    } else if tool == MCP_SEND_MESSAGE_TOOL {
        Some(McpToolWire::Judged(vec![ClientMessage::AgentMessageSend {
            id: 0,
            from_session: String::new(),
            to_session: String::new(),
            text: String::new(),
            idempotency_key: None,
        }]))
    } else if tool == MCP_CREATE_AGENT_TOOL {
        Some(McpToolWire::Judged(vec![
            ClientMessage::SessionCreate {
                id: 0,
                workspace_id: None,
                kind: SessionKind::Claude,
                provider: None,
                mode: None,
                display_name: None,
                idempotency_key: None,
            },
            // The mandatory initial prompt: the same send `devboule_send_message`
            // declares (placeholders: no arm reads a field, only the variant).
            ClientMessage::AgentMessageSend {
                id: 0,
                from_session: String::new(),
                to_session: String::new(),
                text: String::new(),
                idempotency_key: None,
            },
        ]))
    } else if tool == MCP_ANSWER_PERMISSION_TOOL {
        Some(McpToolWire::Judged(vec![
            ClientMessage::SessionPermissionRespond {
                id: 0,
                session_id: String::new(),
                subscription_id: 0,
                request_id: String::new(),
                outcome: devboule_protocol::PermissionOutcome::Deny,
                option_id: None,
                idempotency_key: None,
            },
        ]))
    } else if tool == MCP_SET_AGENT_PROFILE_TOOL {
        Some(McpToolWire::Judged(vec![
            ClientMessage::SessionSetMode {
                id: 0,
                session_id: String::new(),
                mode_id: String::new(),
            },
            ClientMessage::SessionSetModel {
                id: 0,
                session_id: String::new(),
                model_id: None,
                effort: None,
            },
        ]))
    } else if tool == MCP_STOP_AGENT_TOOL {
        // The destructive supervisor verb, judged as the wire's own
        // `SessionStop` — which no capability names, so every peer is
        // refused and the tool stays local-only by construction.
        Some(McpToolWire::Judged(vec![ClientMessage::SessionStop {
            id: 0,
            session_id: String::new(),
            subscription_id: 0,
        }]))
    } else if tool == MCP_CLOSE_AGENT_TOOL {
        // The same construction as the stop tool, for the wire's
        // `SessionClose`: a destructive verb no capability opens.
        Some(McpToolWire::Judged(vec![ClientMessage::SessionClose {
            id: 0,
            session_id: String::new(),
            idempotency_key: None,
        }]))
    } else if tool == MCP_NEIGHBORHOOD_TOOL
        || tool == MCP_IMPORTS_TOOL
        || tool == MCP_IMPORTERS_TOOL
    {
        // The three project-graph tools, local-only by construction.
        //
        // The caller decides the severity, and both categories pass through
        // this one arm. A *local* session already reads the workspace's files
        // (its cwd is the workspace root, and the graph is derived from those
        // files), so the tools hand it nothing it could not read itself. A
        // paired device's agent does not read this machine's files: for it the
        // graph is the project's internal structure travelling the wire. Judged
        // for the more severe of the two, and the wire read is the workspace
        // inventory (`WorkspacesList`), which no capability names - so every
        // peer is refused and the tools stay local by construction, exactly
        // like the stop and close verbs. The graph discloses more per
        // workspace than that inventory does, not less, so it cannot be the
        // read that opens where the inventory is closed. Whose graph is read
        // comes from the caller's own session row, never from an argument.
        Some(McpToolWire::Judged(vec![ClientMessage::WorkspacesList {
            id: 0,
            project_id: String::new(),
        }]))
    } else {
        None
    }
}

/// Judge one tool call for a peer with the same function the dispatcher uses:
/// the policy's first `Deny` payload, or `None` when the tool is allowed.
///
/// `Unjudged` tools and unknown tool names both allow here: the former perform
/// nothing judged, the latter fall through to the broker's own `Unknown tool`
/// refusal, which touches nothing.
pub fn mcp_tool_denial(role: PeerRole, caps: &[String], tool: &str) -> Option<&'static str> {
    let judged = match mcp_tool_wire(tool)? {
        McpToolWire::Judged(requests) => requests,
        McpToolWire::Unjudged(_) => return None,
    };
    for request in &judged {
        if let PeerDecision::Deny(reason) = peer_allows(role, caps, request) {
            return Some(reason);
        }
    }
    None
}

/// Render a policy `Deny` payload the way the wire renders it
/// (`server.rs::capability_not_supported`): the tool refuses with the policy's
/// own sentence, not a paraphrase of it.
pub fn capability_refusal_message(reason: &str) -> String {
    format!("capability '{reason}' was not negotiated")
}

/// What the transport resolved about a peer at connection time. Kept beside
/// the pinned key because the key is the credential and this is the *network*
/// check that must also hold (`DESIGN-remote-agents.md` §8 R10: nothing may
/// depend on `whois` for authentication, only for the binding check).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportBinding {
    /// `"tailnet"` today, `"relay"` later.
    pub kind: String,
    pub stable_id: String,
    pub node_name: String,
    pub login_name: String,
}

impl TransportBinding {
    pub fn tailnet(
        stable_id: impl Into<String>,
        node_name: impl Into<String>,
        login_name: impl Into<String>,
    ) -> Self {
        Self {
            kind: "tailnet".to_string(),
            stable_id: stable_id.into(),
            node_name: node_name.into(),
            login_name: login_name.into(),
        }
    }
}

/// What the daemon knows about the far end of one connection: `Some` is the
/// Noise-authenticated peer, and its `paired_by_user` is copied from the
/// `peers` row at handshake time so no journal lookup happens at dispatch. A
/// pipe connection carries `None`, and its kernel-derived identity stays in
/// `ConnHandle.peer`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnPeer {
    Remote {
        device_id: String,
        role: PeerRole,
        paired_by_user: Option<String>,
        binding: TransportBinding,
    },
}

impl ConnPeer {
    pub fn device_id(&self) -> Option<&str> {
        match self {
            Self::Remote { device_id, .. } => Some(device_id),
        }
    }

    pub fn role(&self) -> Option<PeerRole> {
        match self {
            Self::Remote { role, .. } => Some(*role),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use devboule_protocol::{OwnerId, PromptAttachment};

    fn ping() -> ClientMessage {
        ClientMessage::Ping { id: 1 }
    }

    /// A capability set from names, as the `peers` row holds it.
    fn caps(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// What a freshly paired device holds: `view` and nothing else (§8b A11).
    fn default_caps() -> Vec<String> {
        caps(&[CAP_VIEW])
    }

    /// Every capability the wire set names.
    fn all_caps() -> Vec<String> {
        caps(&[
            CAP_VIEW,
            CAP_SEND,
            CAP_ANSWER_PERMISSIONS,
            CAP_CREATE_SESSIONS,
            CAP_ROSTER,
        ])
    }

    /// The peer roster has a capability of its own, absent from
    /// `PEER_DEFAULT_CAPS`: every pairing holds `view`, and the roster is the
    /// pairing user's whole live surface, so `view` alone must not open it.
    /// Nothing changes for an existing pairing until a person grants this.
    #[test]
    fn the_peer_roster_has_a_capability_of_its_own() {
        let roster = ClientMessage::PeerAgentsList { id: 1 };
        for role in [PeerRole::Client, PeerRole::Daemon] {
            // The default grant (view only) does not open the roster.
            assert_eq!(
                peer_allows(role, &default_caps(), &roster),
                PeerDecision::Deny(CAP_ROSTER),
                "{role:?} holding only the default `view` must not read the roster"
            );
            // Even everything a pairing could hold before this capability
            // existed does not.
            let pre_roster_world = caps(&[
                CAP_VIEW,
                CAP_SEND,
                CAP_ANSWER_PERMISSIONS,
                CAP_CREATE_SESSIONS,
            ]);
            assert_eq!(
                peer_allows(role, &pre_roster_world, &roster),
                PeerDecision::Deny(CAP_ROSTER),
                "{role:?} holding every pre-roster capability must not read the roster"
            );
            // The capability of its own does, and nothing else about it.
            assert_eq!(
                peer_allows(role, &caps(&[CAP_VIEW, CAP_ROSTER]), &roster),
                PeerDecision::Allow
            );
            assert_eq!(
                peer_allows(role, &caps(&[CAP_ROSTER]), &roster),
                PeerDecision::Allow
            );
            // And holding it opens nothing else: the roster read is the act
            // it names.
            let send = ClientMessage::AgentMessageSend {
                id: 1,
                from_session: "s.a.1".to_string(),
                to_session: "s.b.2".to_string(),
                text: "hi".to_string(),
                idempotency_key: None,
            };
            assert_eq!(
                peer_allows(role, &caps(&[CAP_ROSTER]), &send),
                PeerDecision::Deny(CAP_SEND)
            );
        }
    }

    #[test]
    fn the_capability_names_are_the_protocol_list() {
        // The gate enforces these strings; the wire accepts exactly
        // `PEER_CAPS`. Pinning them here means a rename on either side fails
        // loudly instead of leaving a capability nobody can hold.
        let mut named = [
            CAP_VIEW,
            CAP_SEND,
            CAP_ANSWER_PERMISSIONS,
            CAP_CREATE_SESSIONS,
            CAP_ROSTER,
        ];
        named.sort_unstable();
        let mut listed = devboule_protocol::PEER_CAPS;
        listed.sort_unstable();
        assert_eq!(named, listed);
    }

    #[test]
    fn the_allowlist_is_exactly_the_1a_surface_plus_the_slice_3_acts() {
        for role in [PeerRole::Client, PeerRole::Daemon] {
            assert_eq!(
                peer_allows(role, &default_caps(), &ping()),
                PeerDecision::Allow
            );
            assert_eq!(
                peer_allows(
                    role,
                    &default_caps(),
                    &ClientMessage::SessionsList { id: 1 }
                ),
                PeerDecision::Allow
            );
            // §8b A11, H10: the two list acts are reads and `view` is what
            // makes a peer a reader, so a peer holding nothing reaches
            // neither. `Hello` and `Ping` stay unconditional: they are the
            // handshake and the liveness probe the panel needs before any
            // capability question exists.
            let none: Vec<String> = Vec::new();
            assert_eq!(
                peer_allows(role, &none, &ClientMessage::SessionsList { id: 1 }),
                PeerDecision::Deny(CAP_VIEW)
            );
            assert_eq!(
                peer_allows(role, &none, &ClientMessage::DevicesList { id: 1 }),
                PeerDecision::Deny(CAP_VIEW)
            );
            assert_eq!(peer_allows(role, &none, &ping()), PeerDecision::Allow);
        }
    }

    /// One capability, one act. Holding `send` must not open `create`, and
    /// holding nothing must not open a session at all.
    #[test]
    fn each_capability_opens_exactly_the_act_it_names() {
        let create = || ClientMessage::SessionCreate {
            id: 1,
            workspace_id: None,
            kind: SessionKind::Claude,
            provider: None,
            mode: None,
            display_name: None,
            idempotency_key: None,
        };
        let attach = || ClientMessage::SessionAttach {
            id: 1,
            session_id: "s.a.1".to_string(),
            subscription_id: 1,
            from_cursor: None,
        };
        let send = || ClientMessage::SessionSend {
            id: 1,
            session_id: "s.a.1".to_string(),
            subscription_id: 1,
            text: "hi".to_string(),
            attachments: Vec::new(),
            active_turn_behavior: None,
            attachment_references: Vec::new(),
            idempotency_key: None,
        };
        let respond = || ClientMessage::SessionPermissionRespond {
            id: 1,
            session_id: "s.a.1".to_string(),
            subscription_id: 1,
            request_id: "tool-1".to_string(),
            outcome: devboule_protocol::PermissionOutcome::AllowOnce,
            option_id: None,
            idempotency_key: None,
        };
        let set_mode = || ClientMessage::SessionSetMode {
            id: 1,
            session_id: "s.a.1".to_string(),
            mode_id: "acceptEdits".to_string(),
        };

        for role in [PeerRole::Client, PeerRole::Daemon] {
            // No capability at all: every slice-3 act is refused, and the
            // refusal names the capability the peer would need.
            let none: Vec<String> = Vec::new();
            assert_eq!(
                peer_allows(role, &none, &attach()),
                PeerDecision::Deny(CAP_VIEW)
            );
            assert_eq!(
                peer_allows(role, &none, &create()),
                PeerDecision::Deny(CAP_CREATE_SESSIONS)
            );
            assert_eq!(
                peer_allows(role, &none, &send()),
                PeerDecision::Deny(CAP_SEND)
            );
            assert_eq!(
                peer_allows(role, &none, &respond()),
                PeerDecision::Deny(CAP_ANSWER_PERMISSIONS)
            );

            // One capability each, and only its own act.
            assert_eq!(
                peer_allows(role, &caps(&[CAP_VIEW]), &attach()),
                PeerDecision::Allow
            );
            assert_eq!(
                peer_allows(role, &caps(&[CAP_VIEW]), &send()),
                PeerDecision::Deny(CAP_SEND)
            );
            assert_eq!(
                peer_allows(role, &caps(&[CAP_CREATE_SESSIONS]), &create()),
                PeerDecision::Allow
            );
            assert_eq!(
                peer_allows(role, &caps(&[CAP_CREATE_SESSIONS]), &attach()),
                PeerDecision::Deny(CAP_VIEW)
            );
            assert_eq!(
                peer_allows(role, &caps(&[CAP_SEND]), &send()),
                PeerDecision::Allow
            );
            assert_eq!(
                peer_allows(role, &caps(&[CAP_SEND]), &set_mode()),
                PeerDecision::Allow,
                "driving the mode is the act `send` names"
            );
            assert_eq!(
                peer_allows(role, &caps(&[CAP_ANSWER_PERMISSIONS]), &respond()),
                PeerDecision::Allow
            );
            // The full set opens exactly those five and nothing else.
            assert_eq!(
                peer_allows(role, &all_caps(), &create()),
                PeerDecision::Allow
            );
            assert_eq!(
                peer_allows(
                    role,
                    &all_caps(),
                    &ClientMessage::SessionStop {
                        id: 1,
                        session_id: "s.a.1".to_string(),
                        subscription_id: 1,
                    }
                ),
                PeerDecision::Deny("session.stop")
            );
        }
    }

    /// §8 R1's "denied to every role, always" list. The variants are
    /// constructed directly: the compiler already proves the *match* is
    /// exhaustive, this proves the *decisions* on the list that matters.
    #[test]
    fn the_r1_always_denied_list_is_denied_to_both_roles() {
        let denied = [
            ClientMessage::Shutdown { id: 1 },
            ClientMessage::SessionDelete {
                id: 1,
                session_id: "s.a.1".to_string(),
                idempotency_key: None,
            },
            ClientMessage::JournalRetentionSet {
                id: 1,
                max_age_ms: None,
                max_bytes: None,
                max_sessions: None,
                session_max_bytes: None,
                idempotency_key: None,
            },
            ClientMessage::JournalUsage { id: 1 },
            ClientMessage::ProvidersRefresh { id: 1 },
            ClientMessage::ProviderUpdate {
                id: 1,
                provider_id: "claude".to_string(),
            },
            ClientMessage::ProjectAdd {
                id: 1,
                path: "C:\\work".to_string(),
            },
            ClientMessage::WorkspaceCreate {
                id: 1,
                project_id: "p.1".to_string(),
                isolation: devboule_protocol::WorkspaceIsolation::Local,
                branch: None,
            },
            ClientMessage::WorkspaceDelete {
                id: 1,
                workspace_id: "ws.1".to_string(),
                force: false,
            },
            ClientMessage::Invoke {
                id: 1,
                method: "workspace.root".to_string(),
                payload: None,
            },
            ClientMessage::SessionReportAgent {
                id: 1,
                session_id: "s.a.1".to_string(),
                source: "devboule:claude".to_string(),
                agent: "claude".to_string(),
                state: devboule_protocol::AgentActivityState::Working,
                message: None,
                seq: None,
                agent_session_id: None,
                agent_session_path: None,
                session_start_source: None,
            },
            ClientMessage::SessionResume {
                id: 1,
                persistence: devboule_protocol::Persistence {
                    kind: devboule_protocol::PersistenceKind::None,
                },
                idempotency_key: None,
            },
            ClientMessage::SessionClaim {
                id: 1,
                session_id: "s.a.1".to_string(),
                subscription_id: 1,
            },
            // Local acts: a peer that could pair or revoke could change this
            // device's trusted set.
            ClientMessage::PairingStart {
                id: 1,
                role: PeerRole::Client,
            },
            ClientMessage::PairingComplete {
                id: 1,
                address: "100.64.0.2:47831".to_string(),
                code: devboule_protocol::PairingSecret::new("ABCDEFGH"),
                role: PeerRole::Client,
            },
            ClientMessage::PairingConfirm {
                id: 1,
                device_id: "dev-1".to_string(),
                accept: true,
            },
            ClientMessage::PeerRevoke {
                id: 1,
                device_id: "dev-1".to_string(),
            },
            ClientMessage::PeerSetCaps {
                id: 1,
                device_id: "dev-1".to_string(),
                caps: vec!["view".to_string()],
            },
            ClientMessage::ToolPolicyGet { id: 1 },
            ClientMessage::ToolPolicySet {
                id: 1,
                provider_id: "claude".to_string(),
                enabled: Some(false),
                disabled_tools: Vec::new(),
            },
            // The agent profiles, read and write: both are this device's own
            // settings, and a peer holding every capability still gets neither.
            ClientMessage::AgentProfilesGet { id: 1 },
            ClientMessage::AgentProfilesSet {
                id: 1,
                document: devboule_protocol::AgentProfilesDocument::default(),
            },
            // The vocabulary query is the profile store's companion read, and
            // the same refusal: what this machine's providers offer is this
            // machine's own settings material.
            ClientMessage::ProviderVocabularyGet {
                id: 1,
                provider: "claude".to_string(),
                refresh: false,
            },
            // The delegation switch, read and write: the same refusal again.
            // It decides whether this machine's agents may answer their
            // children's permission cards, so a peer holding every capability
            // still gets neither half.
            ClientMessage::DelegationGet { id: 1 },
            ClientMessage::DelegationSet {
                id: 1,
                enabled: true,
            },
        ];
        for role in [PeerRole::Client, PeerRole::Daemon] {
            for request in &denied {
                let decision = peer_allows(role, &all_caps(), request);
                assert!(
                    matches!(decision, PeerDecision::Deny(_)),
                    "{role} may not send {request:?}"
                );
            }
        }
    }

    #[test]
    fn status_is_denied_to_both_roles() {
        for role in [PeerRole::Client, PeerRole::Daemon] {
            let status = ClientMessage::Status { id: 1 };
            let diagnostics = ClientMessage::DaemonDiagnostics { id: 1 };
            assert_eq!(
                peer_allows(role, &all_caps(), &status),
                PeerDecision::Deny("status")
            );
            assert_eq!(
                peer_allows(role, &all_caps(), &diagnostics),
                PeerDecision::Deny("diagnostics")
            );
        }
    }

    #[test]
    fn prompt_skipping_modes_are_the_concrete_per_provider_lists() {
        for mode in ["acceptEdits", "auto", "bypassPermissions"] {
            assert!(prompt_skipping_mode(SessionKind::Claude, mode), "{mode}");
        }
        assert!(!prompt_skipping_mode(SessionKind::Claude, "default"));
        for mode in ["auto-review", "full-access"] {
            assert!(prompt_skipping_mode(SessionKind::Codex, mode), "{mode}");
        }
        assert!(
            !prompt_skipping_mode(SessionKind::Codex, "auto"),
            "Codex `auto` still prompts and is allowed"
        );
        assert!(prompt_skipping_mode(SessionKind::Pi, "bypass"));
        assert!(!prompt_skipping_mode(SessionKind::Pi, "default"));
        assert!(!prompt_skipping_mode(SessionKind::Acp, "any-agent-mode"));
        assert!(!prompt_skipping_mode(SessionKind::Terminal, "anything"));
    }

    /// The `unattended` derivation's three arms, each reachable (R2b): route
    /// A (the broker's own ids), route B (each family's own dictionary), and
    /// the arm the marker exists for — a vocabulary the daemon did not author
    /// answers `unknown`, never `no`, and so does what nobody said.
    #[test]
    fn the_unattended_derivation_has_three_reachable_arms() {
        use devboule_protocol::UnattendedState;
        // Route A: the shared helper, for every agent family, however the
        // mode reached the child.
        for kind in [SessionKind::Acp, SessionKind::Claude, SessionKind::Pi] {
            assert_eq!(
                unattended_mode(kind.clone(), Some("bypass")),
                UnattendedState::Yes,
                "{kind:?}: the daemon's own broker answers this id"
            );
        }
        // Route A does not reach a terminal: the broker is wired for the
        // agent clients only, and a terminal has no permission mechanism at
        // all, so there is no permission moment for anything to answer. The
        // kind match decides it, and the route-A id must not outrank the
        // kind — this is the pair that answered `yes` before the gate.
        assert_eq!(
            unattended_mode(SessionKind::Terminal, Some("bypass")),
            UnattendedState::No,
            "a terminal with a route-A id is still a terminal: no mechanism, \
             nothing to answer, `no`"
        );
        // Route B: Codex `full-access` is the daemon's own knob
        // (`approvalPolicy: never`) while the broker stays silent — the case
        // that proves a two-value, route-A-only marker under-reports.
        assert_eq!(
            unattended_mode(SessionKind::Codex, Some("full-access")),
            UnattendedState::Yes,
            "route B: the daemon authored the knob and it never asks"
        );
        // No: daemon-authored modes that stop at the human, including each
        // family's own default when the create named no mode at all.
        assert_eq!(
            unattended_mode(SessionKind::Claude, None),
            UnattendedState::No
        );
        assert_eq!(
            unattended_mode(SessionKind::Codex, None),
            UnattendedState::No
        );
        assert_eq!(unattended_mode(SessionKind::Pi, None), UnattendedState::No);
        assert_eq!(
            unattended_mode(SessionKind::Claude, Some("acceptEdits")),
            UnattendedState::No
        );
        // `auto-review` is `unknown`, not `no`: the daemon's own peer gate
        // counts the same id as one that can pass a permission moment with
        // nobody answering (`prompt_skipping_mode`), so the silent row `no`
        // renders was the wrongly-benign badge (audit R2b-1 §3.3). It is not
        // `yes` either — a model reviewer may hand a moment back.
        assert_eq!(
            unattended_mode(SessionKind::Codex, Some("auto-review")),
            UnattendedState::Unknown,
            "the peer gate calls this id prompt-skipping, so the marker cannot \
             render the benign nothing"
        );
        // The empty string is the same absence as `None` — filtered out
        // above — so the three authored families answer with their own
        // default mode, each of which stops at the human (audit R2b-1 §3.2:
        // "the empty string for every family is unknown" is not the rule the
        // code applies; this pins the rule the code applies).
        assert_eq!(
            unattended_mode(SessionKind::Claude, Some("")),
            UnattendedState::No
        );
        assert_eq!(
            unattended_mode(SessionKind::Codex, Some("")),
            UnattendedState::No
        );
        assert_eq!(
            unattended_mode(SessionKind::Pi, Some("")),
            UnattendedState::No
        );
        // Unknown: a mode id the daemon did not author. The ACP family is
        // made of them; a miss in a daemon family's own table is the same
        // answer, because an unauthored id is an absence of knowledge and
        // reading it as `no` would claim a human is watching.
        assert_eq!(
            unattended_mode(SessionKind::Acp, Some("agent-authored-mode")),
            UnattendedState::Unknown
        );
        assert_eq!(
            unattended_mode(SessionKind::Claude, Some("sudo-not-a-mode")),
            UnattendedState::Unknown,
            "a dictionary miss is unknown, never no"
        );
        // Unknown: what nobody said. An absent mode for a family the daemon
        // does not set a mode for, and the empty string, are absences of
        // knowledge — the third arm must be reachable from silence too.
        assert_eq!(
            unattended_mode(SessionKind::Acp, None),
            UnattendedState::Unknown
        );
        assert_eq!(
            unattended_mode(SessionKind::Acp, Some("")),
            UnattendedState::Unknown
        );
        // A terminal has no permission mechanism at all, and never had one to
        // be told about: `no`, exactly what the collapsed bool recorded for
        // it, and the roster renders nothing.
        assert_eq!(
            unattended_mode(SessionKind::Terminal, None),
            UnattendedState::No
        );
    }

    /// §8b A5/R3, the ACP half: ACP mode ids belong to the agent, so a paired
    /// device may not name one — for *any* id, including the ones that look
    /// like `ask`. The other kinds keep the concrete list, and its reason.
    #[test]
    fn an_acp_mode_is_never_vettable_for_a_paired_device() {
        for mode in ["ask", "default", "auto_accept", "yolo", ""] {
            assert_eq!(
                mode_refusal(SessionKind::Acp, mode),
                Some(ACP_MODES_UNVETTED_REFUSED),
                "ACP mode {mode:?} cannot be vetted against a list"
            );
        }
        for mode in ["acceptEdits", "auto", "bypassPermissions"] {
            assert_eq!(
                mode_refusal(SessionKind::Claude, mode),
                Some(PROMPT_SKIPPING_REFUSED)
            );
        }
        assert_eq!(mode_refusal(SessionKind::Claude, "default"), None);
        assert_eq!(mode_refusal(SessionKind::Codex, "auto"), None);
        assert_eq!(
            mode_refusal(SessionKind::Codex, "full-access"),
            Some(PROMPT_SKIPPING_REFUSED)
        );
        assert_eq!(
            mode_refusal(SessionKind::Pi, "bypass"),
            Some(PROMPT_SKIPPING_REFUSED)
        );
        assert_eq!(mode_refusal(SessionKind::Pi, "default"), None);
        assert_eq!(
            mode_refusal(SessionKind::Terminal, "bypassPermissions"),
            None
        );
    }

    fn allow() -> PeerDecision {
        PeerDecision::Allow
    }

    /// The P0 tool door's closed table: every tool the broker serves has an arm
    /// in `mcp_tool_wire` — judged, or explicitly unjudged with its reason.
    /// Removing one arm makes this red: the served list (`MCP_BROKER_TOOLS`) is
    /// the same source the broker's `tools/list` reads, so a tool cannot be
    /// served and unjudged at once.
    #[test]
    fn the_mcp_tool_table_covers_every_served_tool() {
        use crate::provider_catalog::MCP_BROKER_TOOLS;
        let mut names: Vec<&str> = Vec::new();
        for (name, _) in MCP_BROKER_TOOLS {
            names.push(name);
            match mcp_tool_wire(name) {
                Some(McpToolWire::Judged(requests)) => assert!(
                    !requests.is_empty(),
                    "{name}: a judged tool names at least one wire act"
                ),
                Some(McpToolWire::Unjudged(reason)) => {
                    assert!(!reason.is_empty(), "{name}: an unjudged tool says why")
                }
                None => panic!("{name}: served by the broker but missing from the door table"),
            }
        }
        // The table the brief convicted on, pinned by name so a rename fails
        // loudly instead of silently unjudging a tool.
        for expected in [
            crate::provider_catalog::MCP_ROSTER_TOOL,
            crate::provider_catalog::MCP_ACTIVITY_TOOL,
            crate::provider_catalog::MCP_LIST_PROFILES_TOOL,
            crate::provider_catalog::MCP_SEND_MESSAGE_TOOL,
            crate::provider_catalog::MCP_CREATE_AGENT_TOOL,
            crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL,
            crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL,
        ] {
            assert!(names.contains(&expected), "{expected} is served");
        }
    }

    /// The verbs that end a child are refused to every peer, walked over the
    /// closed wire capability table rather than sampled: a capability added
    /// to `PEER_CAPS` that could reach stop or close — through the wire gate
    /// or the tool door — fails here by construction.
    #[test]
    fn no_capability_reaches_stop_or_close() {
        use crate::provider_catalog::{MCP_CLOSE_AGENT_TOOL, MCP_STOP_AGENT_TOOL};
        use devboule_protocol::PEER_CAPS;
        let stop = ClientMessage::SessionStop {
            id: 0,
            session_id: String::new(),
            subscription_id: 0,
        };
        let close = ClientMessage::SessionClose {
            id: 0,
            session_id: String::new(),
            idempotency_key: None,
        };
        for role in [PeerRole::Client, PeerRole::Daemon] {
            for cap in PEER_CAPS {
                let caps = caps(&[cap]);
                assert_eq!(
                    peer_allows(role, &caps, &stop),
                    PeerDecision::Deny("session.stop"),
                    "{role:?} holding {cap} must not stop a session"
                );
                assert_eq!(
                    peer_allows(role, &caps, &close),
                    PeerDecision::Deny("session.close"),
                    "{role:?} holding {cap} must not close a session"
                );
                assert_eq!(
                    mcp_tool_denial(role, &caps, MCP_STOP_AGENT_TOOL),
                    Some("session.stop"),
                    "{role:?} holding {cap} must not reach the stop tool"
                );
                assert_eq!(
                    mcp_tool_denial(role, &caps, MCP_CLOSE_AGENT_TOOL),
                    Some("session.close"),
                    "{role:?} holding {cap} must not reach the close tool"
                );
            }
            // One capability at a time is not "every peer": a peer holding the
            // whole table is legal, and a future arm gated on a combination
            // would pass the loop above and fail here.
            let every = caps(&PEER_CAPS);
            assert_eq!(
                peer_allows(role, &every, &stop),
                PeerDecision::Deny("session.stop")
            );
            assert_eq!(
                peer_allows(role, &every, &close),
                PeerDecision::Deny("session.close")
            );
            assert_eq!(
                mcp_tool_denial(role, &every, MCP_STOP_AGENT_TOOL),
                Some("session.stop")
            );
            assert_eq!(
                mcp_tool_denial(role, &every, MCP_CLOSE_AGENT_TOOL),
                Some("session.close")
            );
        }
    }

    /// The door judges with `peer_allows`, per tool, for both roles: the role
    /// never decides (the capability set does), the denials name the policy's
    /// own payloads, and the move tool's two halves deny with two different
    /// sentences — the mode half under `send`, the model half always.
    #[test]
    fn the_mcp_door_denies_with_the_policy_own_sentences() {
        use crate::provider_catalog::*;
        let none: Vec<String> = Vec::new();
        for role in [PeerRole::Client, PeerRole::Daemon] {
            assert_eq!(
                mcp_tool_denial(role, &none, MCP_ROSTER_TOOL),
                Some(CAP_VIEW)
            );
            assert_eq!(
                mcp_tool_denial(role, &caps(&[CAP_VIEW]), MCP_ROSTER_TOOL),
                None
            );
            // The activity read is the roster's act: a peer without `view`
            // is refused, a peer with it reads one agent's metadata.
            assert_eq!(
                mcp_tool_denial(role, &none, MCP_ACTIVITY_TOOL),
                Some(CAP_VIEW)
            );
            assert_eq!(
                mcp_tool_denial(role, &caps(&[CAP_VIEW]), MCP_ACTIVITY_TOOL),
                None
            );
            // The devices read is the roster's act: this daemon's own paired
            // rows, refused without `view`, allowed with it.
            assert_eq!(
                mcp_tool_denial(role, &none, MCP_LIST_DEVICES_TOOL),
                Some(CAP_VIEW)
            );
            assert_eq!(
                mcp_tool_denial(role, &caps(&[CAP_VIEW]), MCP_LIST_DEVICES_TOOL),
                None
            );
            // The peer roster is judged under its own capability, not `view`:
            // every pairing holds `view`, and the roster is the pairing
            // user's whole live surface.
            assert_eq!(
                mcp_tool_denial(role, &none, MCP_LIST_PEER_AGENTS_TOOL),
                Some(CAP_ROSTER)
            );
            assert_eq!(
                mcp_tool_denial(role, &caps(&[CAP_VIEW]), MCP_LIST_PEER_AGENTS_TOOL),
                Some(CAP_ROSTER),
                "{role:?} holding only `view` must not reach the roster tool"
            );
            assert_eq!(
                mcp_tool_denial(role, &caps(&[CAP_ROSTER]), MCP_LIST_PEER_AGENTS_TOOL),
                None
            );
            // The ticked list performs nothing judged: allowed even holding
            // nothing, for both roles.
            assert_eq!(mcp_tool_denial(role, &none, MCP_LIST_PROFILES_TOOL), None);
            assert_eq!(
                mcp_tool_denial(role, &all_caps(), MCP_LIST_PROFILES_TOOL),
                None
            );
            assert_eq!(
                mcp_tool_denial(role, &none, MCP_SEND_MESSAGE_TOOL),
                Some(CAP_SEND)
            );
            assert_eq!(
                mcp_tool_denial(role, &caps(&[CAP_SEND]), MCP_SEND_MESSAGE_TOOL),
                None
            );
            assert_eq!(
                mcp_tool_denial(role, &none, MCP_CREATE_AGENT_TOOL),
                Some(CAP_CREATE_SESSIONS)
            );
            // The create row understated its tool until the re-audit caught it:
            // the tool always sends the mandatory initial prompt, so
            // `create_sessions` without `send` is refused with the policy's own
            // `send` sentence — and `send` without `create_sessions` still meets
            // the create half first.
            assert_eq!(
                mcp_tool_denial(role, &caps(&[CAP_CREATE_SESSIONS]), MCP_CREATE_AGENT_TOOL),
                Some(CAP_SEND)
            );
            assert_eq!(
                mcp_tool_denial(
                    role,
                    &caps(&[CAP_VIEW, CAP_CREATE_SESSIONS]),
                    MCP_CREATE_AGENT_TOOL
                ),
                Some(CAP_SEND),
                "{role:?} that may create but may not talk creates nothing through this tool"
            );
            assert_eq!(
                mcp_tool_denial(
                    role,
                    &caps(&[CAP_CREATE_SESSIONS, CAP_SEND]),
                    MCP_CREATE_AGENT_TOOL
                ),
                None
            );
            assert_eq!(
                mcp_tool_denial(role, &none, MCP_ANSWER_PERMISSION_TOOL),
                Some(CAP_ANSWER_PERMISSIONS)
            );
            assert_eq!(
                mcp_tool_denial(
                    role,
                    &caps(&[CAP_ANSWER_PERMISSIONS]),
                    MCP_ANSWER_PERMISSION_TOOL
                ),
                None
            );
            // The move tool, both halves: without `send` the mode half fires
            // first; with `send` the mode half allows and the model half —
            // denied to every peer, whatever it holds — refuses instead.
            assert_eq!(
                mcp_tool_denial(role, &none, MCP_SET_AGENT_PROFILE_TOOL),
                Some(CAP_SEND)
            );
            assert_eq!(
                mcp_tool_denial(role, &caps(&[CAP_SEND]), MCP_SET_AGENT_PROFILE_TOOL),
                Some("session.set_model")
            );
            assert_eq!(
                mcp_tool_denial(role, &all_caps(), MCP_SET_AGENT_PROFILE_TOOL),
                Some("session.set_model"),
                "{role:?} holding everything is still refused the model half"
            );
            // An unserved name is not the door's refusal: the broker's own
            // `Unknown tool` arm answers it without touching anything.
            assert_eq!(mcp_tool_denial(role, &none, "devboule_no_such_tool"), None);
        }
    }

    /// The wire sentence the door renders a `Deny` payload with is the wire's
    /// own (`server.rs::capability_not_supported`), not a paraphrase.
    #[test]
    fn the_capability_sentence_is_the_wire_sentence() {
        assert_eq!(
            capability_refusal_message("session.set_model"),
            "capability 'session.set_model' was not negotiated"
        );
        assert_eq!(
            capability_refusal_message(CAP_SEND),
            "capability 'send' was not negotiated"
        );
    }

    /// A row for an act no capability names: refused to every set, and the
    /// refusal says why.
    fn always(reason: &'static str) -> (PeerDecision, PeerDecision) {
        (PeerDecision::Deny(reason), PeerDecision::Deny(reason))
    }

    /// A row for an act one capability opens: refused without it, allowed with
    /// every capability held.
    fn under(capability: &'static str) -> (PeerDecision, PeerDecision) {
        (PeerDecision::Deny(capability), PeerDecision::Allow)
    }

    /// §8b A9/A11/A12 as a table: one row per `ClientMessage` variant, holding
    /// the decision a peer with **no** capability gets and the decision a peer
    /// with **all four** gets.
    ///
    /// Closed match with no `_` arm, exactly like `peer_allows` itself: a new
    /// variant does not compile until it has a row here. `VARIANT_COUNT` and
    /// `matrix_samples` below are the other half — they fail the test until the
    /// new variant also has a frame to assert the row on.
    fn matrix_row(request: &ClientMessage) -> (PeerDecision, PeerDecision) {
        match request {
            ClientMessage::Hello(_) | ClientMessage::Ping { .. } => (allow(), allow()),
            ClientMessage::SessionsList { .. } | ClientMessage::DevicesList { .. } => {
                under(CAP_VIEW)
            }
            // The peer roster is a read, but a disclosure of its own: it
            // rides the roster capability, not `view` — every pairing holds
            // `view`, and the roster is the pairing user's whole live
            // surface.
            ClientMessage::PeerAgentsList { .. } => under(CAP_ROSTER),
            ClientMessage::SessionAttach { .. } => under(CAP_VIEW),
            ClientMessage::SessionCreate { .. } => under(CAP_CREATE_SESSIONS),
            ClientMessage::SessionSend { .. } | ClientMessage::SessionSetMode { .. } => {
                under(CAP_SEND)
            }
            // An agent message is a send: it puts text into a session, so it
            // needs the capability `SessionSend` needs and nothing more.
            ClientMessage::AgentMessageSend { .. } => under(CAP_SEND),
            // A deposit is the precursor to a send and holds no opinion of its
            // own, so it reads the same capability: a peer allowed to send must
            // be able to deposit or it can never attach a picture, and a peer
            // not allowed to send must not, or it writes bytes into a session
            // folder nothing on this machine can consume.
            ClientMessage::SessionDeposit { .. } => under(CAP_SEND),
            // The read half of the deposit: refused to every set, like every
            // other content read — a paired device must not read deposited
            // bytes through any capability it holds.
            ClientMessage::SessionAttachmentRead { .. } => always("attachment.read"),
            ClientMessage::SessionPermissionRespond { .. } => under(CAP_ANSWER_PERMISSIONS),
            ClientMessage::Status { .. } => always("status"),
            ClientMessage::DaemonDiagnostics { .. } => always("diagnostics"),
            ClientMessage::Shutdown { .. } => always("shutdown"),
            ClientMessage::SessionDetach { .. } => always("session.detach"),
            ClientMessage::SessionClaim { .. } => always("session.claim"),
            ClientMessage::SessionClose { .. } => always("session.close"),
            ClientMessage::SessionStop { .. } => always("session.stop"),
            ClientMessage::SessionResize { .. } => always("session.resize"),
            ClientMessage::SessionInterrupt { .. } => always("session.interrupt"),
            ClientMessage::SessionSetModel { .. } => always("session.set_model"),
            ClientMessage::SessionReportAgent { .. } => always("session.report_agent"),
            ClientMessage::SessionsWatch { .. } => always("sessions.watch"),
            ClientMessage::SessionsUnwatch { .. } => always("sessions.unwatch"),
            ClientMessage::SessionsPresence { .. } => always("sessions.presence"),
            ClientMessage::SessionResume { .. } => always("session.resume"),
            ClientMessage::SessionDelete { .. } => always("session.delete"),
            ClientMessage::JournalUsage { .. } => always("journal.usage"),
            ClientMessage::JournalRetentionGet { .. } => always("journal.retention.get"),
            ClientMessage::JournalRetentionSet { .. } => always("journal.retention.set"),
            ClientMessage::ProjectsList { .. } => always("projects.list"),
            ClientMessage::ProjectAdd { .. } => always("project.add"),
            ClientMessage::WorkspacesList { .. } => always("workspaces.list"),
            ClientMessage::WorkspaceCreate { .. } => always("workspace.create"),
            ClientMessage::WorkspaceDelete { .. } => always("workspace.delete"),
            ClientMessage::ProvidersList { .. } => always("providers.list"),
            ClientMessage::ProvidersRefresh { .. } => always("providers.refresh"),
            ClientMessage::ProviderUpdate { .. } => always("provider.update"),
            ClientMessage::Invoke { .. } => always("invoke"),
            ClientMessage::PairingStart { .. } => always("pairing.start"),
            ClientMessage::PairingComplete { .. } => always("pairing.complete"),
            ClientMessage::PairingConfirm { .. } => always("pairing.confirm"),
            ClientMessage::PeerRevoke { .. } => always("peer.revoke"),
            ClientMessage::PeerSetCaps { .. } => always("peer.set_caps"),
            ClientMessage::ToolPolicyGet { .. } => always("tool.policy.get"),
            ClientMessage::ToolPolicySet { .. } => always("tool.policy.set"),
            ClientMessage::AgentProfilesGet { .. } => always("agent.profiles.get"),
            ClientMessage::AgentProfilesSet { .. } => always("agent.profiles.set"),
            ClientMessage::ProviderVocabularyGet { .. } => always("provider.vocabulary.get"),
            ClientMessage::DelegationGet { .. } => always("delegation.get"),
            ClientMessage::DelegationSet { .. } => always("delegation.set"),
        }
    }

    /// The number of `ClientMessage` variants at this commit. The closed match
    /// in `every_variant_is_listed` breaks the build when a variant is added;
    /// this number is what then fails
    /// `the_capability_matrix_covers_every_client_frame` until the new variant
    /// also has a sample to assert its row on. Both halves are needed: the
    /// match proves the *decisions* are complete, the count proves the
    /// *frames* are.
    pub(crate) const VARIANT_COUNT: usize = 53;

    /// The wire name of every variant, as a closed match with no `_` arm: the
    /// compile-time half of the matrix. The test compares each arm against
    /// `ClientMessage::name()`, so a mistyped arm is a red test rather than a
    /// silent hole.
    fn every_variant_is_listed(request: &ClientMessage) -> &'static str {
        match request {
            ClientMessage::Hello(_) => "Hello",
            ClientMessage::Ping { .. } => "Ping",
            ClientMessage::Status { .. } => "Status",
            ClientMessage::DaemonDiagnostics { .. } => "DaemonDiagnostics",
            ClientMessage::Shutdown { .. } => "Shutdown",
            ClientMessage::SessionCreate { .. } => "SessionCreate",
            ClientMessage::SessionAttach { .. } => "SessionAttach",
            ClientMessage::SessionDetach { .. } => "SessionDetach",
            ClientMessage::SessionClaim { .. } => "SessionClaim",
            ClientMessage::SessionClose { .. } => "SessionClose",
            ClientMessage::SessionStop { .. } => "SessionStop",
            ClientMessage::SessionSend { .. } => "SessionSend",
            ClientMessage::SessionDeposit { .. } => "SessionDeposit",
            ClientMessage::SessionAttachmentRead { .. } => "SessionAttachmentRead",
            ClientMessage::AgentMessageSend { .. } => "AgentMessageSend",
            ClientMessage::SessionResize { .. } => "SessionResize",
            ClientMessage::SessionInterrupt { .. } => "SessionInterrupt",
            ClientMessage::SessionSetModel { .. } => "SessionSetModel",
            ClientMessage::SessionSetMode { .. } => "SessionSetMode",
            ClientMessage::SessionPermissionRespond { .. } => "SessionPermissionRespond",
            ClientMessage::SessionReportAgent { .. } => "SessionReportAgent",
            ClientMessage::SessionsList { .. } => "SessionsList",
            ClientMessage::SessionsWatch { .. } => "SessionsWatch",
            ClientMessage::SessionsUnwatch { .. } => "SessionsUnwatch",
            ClientMessage::SessionsPresence { .. } => "SessionsPresence",
            ClientMessage::SessionResume { .. } => "SessionResume",
            ClientMessage::JournalUsage { .. } => "JournalUsage",
            ClientMessage::JournalRetentionGet { .. } => "JournalRetentionGet",
            ClientMessage::JournalRetentionSet { .. } => "JournalRetentionSet",
            ClientMessage::SessionDelete { .. } => "SessionDelete",
            ClientMessage::ProjectsList { .. } => "ProjectsList",
            ClientMessage::ProjectAdd { .. } => "ProjectAdd",
            ClientMessage::WorkspacesList { .. } => "WorkspacesList",
            ClientMessage::WorkspaceCreate { .. } => "WorkspaceCreate",
            ClientMessage::WorkspaceDelete { .. } => "WorkspaceDelete",
            ClientMessage::ProvidersList { .. } => "ProvidersList",
            ClientMessage::ProvidersRefresh { .. } => "ProvidersRefresh",
            ClientMessage::ProviderUpdate { .. } => "ProviderUpdate",
            ClientMessage::Invoke { .. } => "Invoke",
            ClientMessage::DevicesList { .. } => "DevicesList",
            ClientMessage::PeerAgentsList { .. } => "PeerAgentsList",
            ClientMessage::PairingStart { .. } => "PairingStart",
            ClientMessage::PairingComplete { .. } => "PairingComplete",
            ClientMessage::PairingConfirm { .. } => "PairingConfirm",
            ClientMessage::PeerRevoke { .. } => "PeerRevoke",
            ClientMessage::PeerSetCaps { .. } => "PeerSetCaps",
            ClientMessage::ToolPolicyGet { .. } => "ToolPolicyGet",
            ClientMessage::ToolPolicySet { .. } => "ToolPolicySet",
            ClientMessage::AgentProfilesGet { .. } => "AgentProfilesGet",
            ClientMessage::AgentProfilesSet { .. } => "AgentProfilesSet",
            ClientMessage::ProviderVocabularyGet { .. } => "ProviderVocabularyGet",
            ClientMessage::DelegationGet { .. } => "DelegationGet",
            ClientMessage::DelegationSet { .. } => "DelegationSet",
        }
    }

    /// One frame per variant, in `name()` order.
    pub(crate) fn matrix_samples() -> Vec<ClientMessage> {
        let owner = OwnerId::new("S-1-5-21-1", "client").expect("owner");
        vec![
            ClientMessage::Hello(devboule_protocol::ClientHello::m3a(owner, "devboule-test")),
            ClientMessage::Ping { id: 1 },
            ClientMessage::Status { id: 1 },
            ClientMessage::DaemonDiagnostics { id: 1 },
            ClientMessage::Shutdown { id: 1 },
            ClientMessage::SessionCreate {
                id: 1,
                workspace_id: None,
                kind: SessionKind::Claude,
                provider: None,
                mode: None,
                display_name: None,
                idempotency_key: None,
            },
            ClientMessage::SessionAttach {
                id: 1,
                session_id: "s.a.1".to_string(),
                subscription_id: 1,
                from_cursor: None,
            },
            ClientMessage::SessionDetach {
                id: 1,
                session_id: "s.a.1".to_string(),
                subscription_id: 1,
            },
            ClientMessage::SessionClaim {
                id: 1,
                session_id: "s.a.1".to_string(),
                subscription_id: 1,
            },
            ClientMessage::SessionClose {
                id: 1,
                session_id: "s.a.1".to_string(),
                idempotency_key: None,
            },
            ClientMessage::SessionStop {
                id: 1,
                session_id: "s.a.1".to_string(),
                subscription_id: 1,
            },
            ClientMessage::SessionSend {
                id: 1,
                session_id: "s.a.1".to_string(),
                subscription_id: 1,
                text: "hi".to_string(),
                attachments: Vec::new(),
                active_turn_behavior: None,
                attachment_references: Vec::new(),
                idempotency_key: None,
            },
            ClientMessage::SessionDeposit {
                id: 1,
                session_id: "s.a.1".to_string(),
                attachment: PromptAttachment {
                    name: "page.png".to_string(),
                    mime_type: "image/png".to_string(),
                    data: String::new(),
                },
            },
            ClientMessage::SessionAttachmentRead {
                id: 1,
                reference: devboule_protocol::AttachmentReference {
                    session_id: "s.a.1".to_string(),
                    digest: "b".repeat(64),
                    stored_bytes: 512,
                },
            },
            ClientMessage::AgentMessageSend {
                id: 1,
                from_session: "s.a.1".to_string(),
                to_session: "s.b.2".to_string(),
                text: "hi".to_string(),
                idempotency_key: None,
            },
            ClientMessage::SessionResize {
                id: 1,
                session_id: "s.a.1".to_string(),
                subscription_id: 1,
                cols: 80,
                rows: 24,
            },
            ClientMessage::SessionInterrupt {
                id: 1,
                session_id: "s.a.1".to_string(),
                subscription_id: 1,
            },
            ClientMessage::SessionSetModel {
                id: 1,
                session_id: "s.a.1".to_string(),
                model_id: None,
                effort: None,
            },
            ClientMessage::SessionSetMode {
                id: 1,
                session_id: "s.a.1".to_string(),
                mode_id: "acceptEdits".to_string(),
            },
            ClientMessage::SessionPermissionRespond {
                id: 1,
                session_id: "s.a.1".to_string(),
                subscription_id: 1,
                request_id: "tool-1".to_string(),
                outcome: devboule_protocol::PermissionOutcome::AllowOnce,
                option_id: None,
                idempotency_key: None,
            },
            ClientMessage::SessionReportAgent {
                id: 1,
                session_id: "s.a.1".to_string(),
                source: "devboule:claude".to_string(),
                agent: "claude".to_string(),
                state: devboule_protocol::AgentActivityState::Working,
                message: None,
                seq: None,
                agent_session_id: None,
                agent_session_path: None,
                session_start_source: None,
            },
            ClientMessage::SessionsList { id: 1 },
            ClientMessage::SessionsWatch { id: 1 },
            ClientMessage::SessionsUnwatch { id: 1 },
            ClientMessage::SessionsPresence {
                id: 1,
                focused_session_id: None,
                app_visible: true,
            },
            ClientMessage::SessionResume {
                id: 1,
                persistence: devboule_protocol::Persistence {
                    kind: devboule_protocol::PersistenceKind::None,
                },
                idempotency_key: None,
            },
            ClientMessage::JournalUsage { id: 1 },
            ClientMessage::JournalRetentionGet { id: 1 },
            ClientMessage::JournalRetentionSet {
                id: 1,
                max_age_ms: None,
                max_bytes: None,
                max_sessions: None,
                session_max_bytes: None,
                idempotency_key: None,
            },
            ClientMessage::SessionDelete {
                id: 1,
                session_id: "s.a.1".to_string(),
                idempotency_key: None,
            },
            ClientMessage::ProjectsList { id: 1 },
            ClientMessage::ProjectAdd {
                id: 1,
                path: "C:\\work".to_string(),
            },
            ClientMessage::WorkspacesList {
                id: 1,
                project_id: "p.1".to_string(),
            },
            ClientMessage::WorkspaceCreate {
                id: 1,
                project_id: "p.1".to_string(),
                isolation: devboule_protocol::WorkspaceIsolation::Local,
                branch: None,
            },
            ClientMessage::WorkspaceDelete {
                id: 1,
                workspace_id: "ws.1".to_string(),
                force: false,
            },
            ClientMessage::ProvidersList { id: 1 },
            ClientMessage::ProvidersRefresh { id: 1 },
            ClientMessage::ProviderUpdate {
                id: 1,
                provider_id: "claude".to_string(),
            },
            ClientMessage::Invoke {
                id: 1,
                method: "workspace.root".to_string(),
                payload: None,
            },
            ClientMessage::DevicesList { id: 1 },
            ClientMessage::PeerAgentsList { id: 1 },
            ClientMessage::PairingStart {
                id: 1,
                role: PeerRole::Client,
            },
            ClientMessage::PairingComplete {
                id: 1,
                address: "100.64.0.2:47831".to_string(),
                code: devboule_protocol::PairingSecret::new("ABCDEFGH"),
                role: PeerRole::Client,
            },
            ClientMessage::PairingConfirm {
                id: 1,
                device_id: "dev-1".to_string(),
                accept: true,
            },
            ClientMessage::PeerRevoke {
                id: 1,
                device_id: "dev-1".to_string(),
            },
            ClientMessage::PeerSetCaps {
                id: 1,
                device_id: "dev-1".to_string(),
                caps: vec!["view".to_string()],
            },
            ClientMessage::ToolPolicyGet { id: 1 },
            ClientMessage::ToolPolicySet {
                id: 1,
                provider_id: "claude".to_string(),
                enabled: Some(false),
                disabled_tools: Vec::new(),
            },
            ClientMessage::AgentProfilesGet { id: 1 },
            ClientMessage::AgentProfilesSet {
                id: 1,
                document: devboule_protocol::AgentProfilesDocument::default(),
            },
            ClientMessage::ProviderVocabularyGet {
                id: 1,
                provider: "claude".to_string(),
                refresh: false,
            },
            ClientMessage::DelegationGet { id: 1 },
            ClientMessage::DelegationSet {
                id: 1,
                enabled: false,
            },
        ]
    }

    /// §8b A9/A11/A12 end to end: every frame, both roles, no capability and
    /// every capability, against the table above. The table is closed by the
    /// compiler and the frame list is pinned by `VARIANT_COUNT`, so a new
    /// variant cannot arrive without a decision *and* a frame to check it on.
    #[test]
    fn the_capability_matrix_covers_every_client_frame() {
        let samples = matrix_samples();
        assert_eq!(samples.len(), VARIANT_COUNT, "one sample per variant");
        let mut names: Vec<&'static str> = samples.iter().map(every_variant_is_listed).collect();
        names.sort_unstable();
        let mut unique = names.clone();
        unique.dedup();
        assert_eq!(names, unique, "one sample per variant, no duplicates");
        let none: Vec<String> = Vec::new();
        for sample in &samples {
            assert_eq!(
                every_variant_is_listed(sample),
                sample.name(),
                "the matrix arm must spell the wire name"
            );
            let (without_caps, with_all) = matrix_row(sample);
            for role in [PeerRole::Client, PeerRole::Daemon] {
                assert_eq!(
                    peer_allows(role, &none, sample),
                    without_caps,
                    "{role} with no capability on {}",
                    sample.name()
                );
                assert_eq!(
                    peer_allows(role, &all_caps(), sample),
                    with_all,
                    "{role} with every capability on {}",
                    sample.name()
                );
            }
        }
    }

    #[test]
    fn roles_round_trip_through_their_wire_names() {
        for role in [PeerRole::Client, PeerRole::Daemon] {
            assert_eq!(PeerRole::parse(role.as_str()), Some(role));
        }
        assert_eq!(PeerRole::parse("admin"), None);
        assert_eq!(PeerRole::Client.as_str(), "client");
        assert_eq!(PeerRole::Daemon.as_str(), "daemon");
        // The wire spelling and `as_str` cannot drift: both are asserted on
        // the serialised form, not on the Rust variant.
        assert_eq!(
            serde_json::to_value(PeerRole::Client).expect("json"),
            serde_json::json!("client")
        );
        assert_eq!(
            serde_json::to_value(PeerRole::Daemon).expect("json"),
            serde_json::json!("daemon")
        );
    }

    #[test]
    fn a_peer_may_not_read_a_deposited_attachment() {
        // The read half of the deposit, refused like every other content
        // read: no capability a pairing can hold opens deposited bytes to
        // a paired device.
        let read = || ClientMessage::SessionAttachmentRead {
            id: 1,
            reference: devboule_protocol::AttachmentReference {
                session_id: "s.a.1".to_string(),
                digest: "b".repeat(64),
                stored_bytes: 512,
            },
        };
        for role in [PeerRole::Client, PeerRole::Daemon] {
            assert_eq!(
                peer_allows(role, &Vec::new(), &read()),
                PeerDecision::Deny("attachment.read")
            );
            assert_eq!(
                peer_allows(role, &all_caps(), &read()),
                PeerDecision::Deny("attachment.read"),
                "{role:?} holding everything is still refused the read"
            );
        }
    }

    #[test]
    fn a_peer_may_read_the_device_list_but_may_not_change_the_trusted_set() {
        for role in [PeerRole::Client, PeerRole::Daemon] {
            let devices = ClientMessage::DevicesList { id: 1 };
            assert_eq!(
                peer_allows(role, &all_caps(), &devices),
                PeerDecision::Allow
            );
            for request in [
                ClientMessage::PairingStart {
                    id: 1,
                    role: PeerRole::Client,
                },
                ClientMessage::PairingConfirm {
                    id: 1,
                    device_id: "dev-1".to_string(),
                    accept: true,
                },
                ClientMessage::PeerRevoke {
                    id: 1,
                    device_id: "dev-1".to_string(),
                },
                ClientMessage::PeerSetCaps {
                    id: 1,
                    device_id: "dev-1".to_string(),
                    caps: vec!["view".to_string()],
                },
            ] {
                let decision = peer_allows(role, &all_caps(), &request);
                assert!(
                    matches!(decision, PeerDecision::Deny(_)),
                    "{role} may not send {request:?}"
                );
            }
        }
    }

    #[test]
    fn a_remote_conn_carries_its_device_and_role() {
        let peer = ConnPeer::Remote {
            device_id: "dev-1".to_string(),
            role: PeerRole::Daemon,
            paired_by_user: Some("S-1-5-21-1".to_string()),
            binding: TransportBinding::tailnet("nstable", "node", "user@example.com"),
        };
        assert_eq!(peer.device_id(), Some("dev-1"));
        assert_eq!(peer.role(), Some(PeerRole::Daemon));
    }

    #[test]
    fn the_owner_id_for_a_remote_peer_uses_an_underscore() {
        // §8b A2: `peer_<device_id>` is the only spelling `is_id_alphabet`
        // accepts. Constructed here so a future rename cannot drift from it.
        // The `p`-prefixed session token is S5.
        let owner = OwnerId::new("peer_dev-1", "daemon").expect("remote owner token");
        assert_eq!(owner.user, "peer_dev-1");
        assert_eq!(owner.client, "daemon");
    }

    /// Both origins share one figure until a device is measured: the peer case
    /// is the local derivation reused, not a second invention.
    #[test]
    fn both_origins_share_the_derived_attachment_budget() {
        // 200 rendered PDF pages at 96 KiB plus one frame of inline
        // attachments — the figure the brief names as 20 MiB.
        let derived = BUDGET_PAGES_PER_TURN * BUDGET_BYTES_PER_PAGE + BUDGET_INLINE_FRAME_BYTES;
        assert_eq!(budget_for(&SessionOrigin::local()), derived);
        assert_eq!(
            budget_for(&SessionOrigin::peer("device-phone", PeerRole::Client)),
            derived
        );
    }
}
