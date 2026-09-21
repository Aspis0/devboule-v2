//! Peer policy: what a paired connection may do, in one closed place.
//!
//! The decision is a **closed match with no `_` arm** over every
//! `ClientMessage` variant. Adding a variant without deciding its peer policy
//! is a compile error, which is the exhaustiveness guarantee the design asks
//! for (`DESIGN-remote-agents.md` §8 R2, §8b A1/A12). A runtime
//! `ALL_SAMPLES` iteration would only restate what the compiler already
//! enforces.
//!
//! The operational surface was opened in slices, each act under the capability
//! that names it (`view`, `send`, `answer_permissions`, `create_sessions`,
//! `roster` — §8b A9/A11/A12). Everything else used to be refused to every
//! peer, whatever it held, because no capability named those acts. **That rule
//! was revoked on 2026-09-21 by the owner**: a paired device is a full client,
//! and the sixth capability, `admin`, opens the remainder. The one thing that
//! stays local is the permission model itself — pairing, a device's capability
//! set, and revocation — because a peer that could change this device's
//! trusted set could let itself in. Scope — *which* sessions an allowed request
//! reaches — is not decided here: it is the owner projection in `server.rs`
//! plus the origin branch of `check_user_owner`.

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
/// pins these six to it so a rename cannot leave the gate enforcing a
/// capability nobody can hold.
pub const CAP_VIEW: &str = "view";
/// `send` names two target scopes: `SessionSend` keeps the peer's existing
/// own-origin scope, while `AgentMessageSend` lets a daemon-role peer write
/// only into local sessions of the user who paired that device. The frame's
/// target rule enforces the latter; this capability still gates the act.
pub const CAP_SEND: &str = "send";
pub const CAP_ANSWER_PERMISSIONS: &str = "answer_permissions";
pub const CAP_CREATE_SESSIONS: &str = "create_sessions";
/// The peer roster (`PeerAgentsList`) has a capability of its own, and no other
/// act's capability opens it: `view` is the weakest grant a pairing can carry,
/// and the roster is the pairing user's whole live surface, so reading it stays
/// a decision a person can take back on its own row. It is part of
/// `PEER_DEFAULT_CAPS` since the 2026-09-21 parity decision — a new device is
/// born with it, and the per-device switch is how it comes off.
pub const CAP_ROSTER: &str = "roster";
/// The administrative capability: the rest of this device's surface, which no
/// operational capability names — the settings stores, the journal, projects
/// and workspaces, the provider verbs, `Status` and diagnostics, `Shutdown`,
/// the session verbs outside the view/send pair, the deposited-bytes read, and
/// the tool bridge's destructive tools. It is named for the surface and not for
/// an act, because it is the whole remainder: one switch a person can turn off
/// to bring a device back to the act-named capabilities alone. A grant, not a
/// shortcut — ownership is unchanged, so an allowed request still has to reach
/// the session its own scope names.
pub const CAP_ADMIN: &str = "admin";

/// The audit outcome for a request refused because it would run a session
/// without asking the user's permission (`DESIGN-remote-agents.md` §8b A5).
/// One spelling, used by the refusal and by the audit row it writes.
pub const PROMPT_SKIPPING_REFUSED: &str = "prompt_skipping_refused";

/// The audit outcome for a request refused because it reached for an ACP
/// session mode (`DESIGN-remote-agents.md` §8b A5, §8 R3): ACP mode ids are
/// defined by the agent at run time, so there is no list to vet them against
/// and a paired device may not choose one at all.
pub const ACP_MODES_UNVETTED_REFUSED: &str = "acp_modes_unvetted_refused";

/// What a peer is told when it names an ACP mode. One spelling: `mode_refusal`
/// produces the reason, and the peer gate (`server/peer_gate.rs`) renders it.
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
/// `caps` is the peer's own capability set, read from its `peers` row. Six
/// names are the whole permission model for a paired device (§8b A9/A11):
/// `view` (`SessionsList`, `DevicesList`, `SessionAttach`), `send`
/// (`SessionSend`, `AgentMessageSend`, `SessionDeposit`, `SessionSetMode`),
/// `answer_permissions` (`SessionPermissionRespond`), `create_sessions`
/// (`SessionCreate`), `roster` (`PeerAgentsList`) and `admin` (everything else
/// this device's app can ask — see [`CAP_ADMIN`], which is the 2026-09-21
/// revocation of the old global deny list). A variant an operational capability
/// names is allowed exactly when the peer holds it; a variant no operational
/// capability names is allowed exactly when the peer holds `admin`. The five
/// permission-model variants — pairing, a device's capability set, revocation —
/// are refused to every set, and that is the whole remaining `Deny`.
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
        // The two list acts are reads, and both ride `view` (§8b A11):
        // `SessionsList` and `DevicesList` are how a paired device sees
        // anything at all, so a peer stripped of `view` — only a
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

        // The seven session variants a paired device may reach — five opened by
        // slice 3, with the deposit and agent-message arms joining later — each
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

        // The permission model itself is the one local act left. Starting or
        // completing a pairing, changing a device's capability set and revoking
        // a device are not uses of this machine, they decide *who may enter*
        // it: a peer that could pair another device could hand out the
        // permissions it holds, and one that could change caps or revoke could
        // rewrite the trusted set. `admin` does not open them, deliberately and
        // on the owner's instruction (2026-09-21) — if that changes, it is
        // their decision to make, and this is the comment that must move with
        // it.
        ClientMessage::PairingStart { .. } => PeerDecision::Deny("pairing.start"),
        ClientMessage::PairingComplete { .. } => PeerDecision::Deny("pairing.complete"),
        ClientMessage::PairingConfirm { .. } => PeerDecision::Deny("pairing.confirm"),
        ClientMessage::PeerRevoke { .. } => PeerDecision::Deny("peer.revoke"),
        ClientMessage::PeerSetCaps { .. } => PeerDecision::Deny("peer.set_caps"),
        // The read half of the deposit. Reads of *content* ride the
        // administrative capability while only the list reads ride `view`:
        // deposited bytes are a session's own material, and a device the owner
        // granted the whole surface may read them exactly as the app does.
        // Scope still decides which reference resolves.
        ClientMessage::SessionAttachmentRead { .. } => with_capability(caps, CAP_ADMIN),

        // The settings stores: the tool policies, the agent profiles (mode,
        // model, tool overlay, and the standing instructions that travel into
        // every created agent's prompt) and the delegation switch — whether an
        // agent here may answer its child's permission cards. They are this
        // device's own settings, read and written by its user through the app,
        // so they ride the administrative capability: a device the owner
        // granted the whole surface may change what this machine hands to its
        // agents, and a device without it may not.
        ClientMessage::ToolPolicyGet { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::ToolPolicySet { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::AgentProfilesGet { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::AgentProfilesSet { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::DelegationGet { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::DelegationSet { .. } => with_capability(caps, CAP_ADMIN),
        // The provider vocabulary is the profile store's companion read — what
        // this machine's providers offer — so the same capability opens it. No
        // handshake name decides this arm: the app's `provider_vocabulary`
        // name is a frame-compatibility gate, a different mechanism.
        ClientMessage::ProviderVocabularyGet { .. } => with_capability(caps, CAP_ADMIN),

        // Local information: pid, instance id, live counts and the secret-store
        // selector. `Ping` and `DevicesList.self_info` remain what a peer uses
        // to know this daemon is alive (muse M7, §8b A13); the full answer is
        // part of the surface `admin` opens.
        ClientMessage::Status { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::DaemonDiagnostics { .. } => with_capability(caps, CAP_ADMIN),

        // The R1 list of §8 — destructive and configuration changes — is no
        // longer denied to every peer. A device the owner granted the
        // administrative capability asks these exactly as the app does; one
        // that does not is refused with the name of the capability it is
        // missing, not with the act's name, which is no longer what would open
        // it. `Invoke` is the generic door of this wire variant: see its own
        // note below.
        ClientMessage::Shutdown { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::SessionDelete { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::JournalRetentionSet { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::ProvidersRefresh { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::ProviderUpdate { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::ProjectAdd { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::WorkspaceCreate { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::WorkspaceDelete { .. } => with_capability(caps, CAP_ADMIN),
        // `Invoke` is the wire's generic tenant door — "run this command" — and
        // the daemon's own arm answers it with `Unimplemented`
        // (`dispatch.rs`), because this daemon is not a plugin backend. What the
        // gate opens here is that refusal, not the app's command surface; if a
        // future slice serves `Invoke` from this daemon, a peer holding `admin`
        // reaches whatever it serves. Said plainly rather than left to be
        // discovered.
        ClientMessage::Invoke { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::SessionClaim { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::SessionResume { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::SessionReportAgent { .. } => with_capability(caps, CAP_ADMIN),

        // The verbs that were "still outside a peer's reach after slice 3":
        // the same capability, for the same reason — the administrative
        // surface is one grant, not thirty-six.
        ClientMessage::JournalUsage { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::JournalRetentionGet { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::SessionDetach { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::SessionClose { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::SessionStop { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::SessionResize { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::SessionInterrupt { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::SessionSetModel { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::SessionsWatch { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::SessionsUnwatch { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::SessionsPresence { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::ProjectsList { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::WorkspacesList { .. } => with_capability(caps, CAP_ADMIN),
        ClientMessage::ProvidersList { .. } => with_capability(caps, CAP_ADMIN),
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
///   standing instructions), which rides the administrative capability and is
///   served by no tool.
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
///   Since the model half rides the administrative capability, a peer applies a
///   profile only when it holds `admin`; a peer with `send` alone may still
///   change modes over the wire `SessionSetMode`.
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
            "the ticked subset the human enabled for agents; the full-document read rides the administrative capability and no tool serves it",
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
        // `SessionStop`: it rides the administrative capability, so a device
        // the owner granted the whole surface may stop a child through this
        // door exactly as it may over the wire, and a device without `admin`
        // is refused with that capability's name.
        Some(McpToolWire::Judged(vec![ClientMessage::SessionStop {
            id: 0,
            session_id: String::new(),
            subscription_id: 0,
        }]))
    } else if tool == MCP_CLOSE_AGENT_TOOL {
        // The same construction as the stop tool, for the wire's
        // `SessionClose`: the same administrative capability opens it.
        Some(McpToolWire::Judged(vec![ClientMessage::SessionClose {
            id: 0,
            session_id: String::new(),
            idempotency_key: None,
        }]))
    } else if tool == MCP_NEIGHBORHOOD_TOOL
        || tool == MCP_IMPORTS_TOOL
        || tool == MCP_IMPORTERS_TOOL
    {
        // The three project-graph tools.
        //
        // The caller decides the severity, and both categories pass through
        // this one arm. A *local* session already reads the workspace's files
        // (its cwd is the workspace root, and the graph is derived from those
        // files), so the tools hand it nothing it could not read itself. A
        // paired device's agent does not read this machine's files: for it the
        // graph is the project's internal structure travelling the wire. Judged
        // for the more severe of the two, and the wire read is the workspace
        // inventory (`WorkspacesList`) — the read the administrative capability
        // opens. It must be *that* read and not `view`: the graph discloses more
        // per workspace than the inventory does, not less, so the graph cannot
        // ride the weaker capability the inventory is closed to. Whose graph is
        // read comes from the caller's own session row, never from an argument.
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

    /// What a freshly paired device holds: `PEER_DEFAULT_CAPS` itself, which
    /// since the 2026-09-21 parity decision is the whole wire set.
    fn default_caps() -> Vec<String> {
        caps(&devboule_protocol::PEER_DEFAULT_CAPS)
    }

    /// Every capability the wire set names — read from the wire set rather than
    /// respelled here, so a seventh name cannot be added to the protocol and
    /// stay invisible to the gate's own tests.
    fn all_caps() -> Vec<String> {
        caps(&devboule_protocol::PEER_CAPS)
    }

    /// Every capability **except** the administrative one: the five that name
    /// acts. The negative control's set — a device holding all of them and
    /// nothing else must still be refused the administrative surface.
    fn operational_caps() -> Vec<String> {
        let mut names = all_caps();
        names.retain(|cap| cap != CAP_ADMIN);
        assert_eq!(
            names.len(),
            all_caps().len() - 1,
            "the administrative name must be in the wire set"
        );
        names
    }

    /// The peer roster has a capability of its own, and `view` — the weakest
    /// grant a pairing can carry — does not open it. Since the parity decision
    /// a new pairing is *born* holding it, so both halves are pinned: the
    /// default carries it, and losing it is enough to close the read.
    #[test]
    fn the_peer_roster_has_a_capability_of_its_own() {
        let roster = ClientMessage::PeerAgentsList { id: 1 };
        for role in [PeerRole::Client, PeerRole::Daemon] {
            // A new pairing holds it, because the default is the whole set.
            assert_eq!(
                peer_allows(role, &default_caps(), &roster),
                PeerDecision::Allow,
                "{role:?} holding the default grant must read the roster"
            );
            // `view` alone does not open it, and neither does every other
            // capability together — `admin` included: the roster read is the
            // one act only its own name reaches, which is why it keeps a switch
            // of its own.
            assert_eq!(
                peer_allows(role, &caps(&[CAP_VIEW]), &roster),
                PeerDecision::Deny(CAP_ROSTER),
                "{role:?} holding only `view` must not read the roster"
            );
            let mut without_roster = all_caps();
            without_roster.retain(|cap| cap != CAP_ROSTER);
            assert_eq!(
                peer_allows(role, &without_roster, &roster),
                PeerDecision::Deny(CAP_ROSTER),
                "{role:?} holding every other capability must not read the roster"
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
            CAP_ADMIN,
        ];
        named.sort_unstable();
        let mut listed = devboule_protocol::PEER_CAPS;
        listed.sort_unstable();
        assert_eq!(named, listed);
    }

    #[test]
    fn the_handshake_pair_is_unconditional_and_view_makes_a_reader() {
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

    /// One capability, one act. Holding `send` must not open `create`, holding
    /// nothing must not open a session at all, and the five act-named
    /// capabilities together still do not open an act none of them names.
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
            // The operational five open exactly the acts they name — and stop
            // is not one of them, whatever combination of act-names is held.
            assert_eq!(
                peer_allows(role, &all_caps(), &create()),
                PeerDecision::Allow
            );
            assert_eq!(
                peer_allows(
                    role,
                    &operational_caps(),
                    &ClientMessage::SessionStop {
                        id: 1,
                        session_id: "s.a.1".to_string(),
                        subscription_id: 1,
                    }
                ),
                PeerDecision::Deny(CAP_ADMIN)
            );
        }
    }

    /// Every variant that used to be "denied to every role, always" (§8 R1), in
    /// one list: the parity walk's subject.
    ///
    /// The list is the old one verbatim — that is the point. **This is the
    /// parity proof at the wire level**: against the default grant of a new
    /// pairing every entry that is not one of the three permission-model acts
    /// must come back `Allow`, and against a device holding every *operational*
    /// capability and no `admin`, `Deny(CAP_ADMIN)` — the negative control. The
    /// variants are constructed directly: the compiler already proves the
    /// *match* is exhaustive, this proves the *decisions* on the list that
    /// matters.
    #[test]
    fn the_administrative_surface_opens_with_the_administrative_capability() {
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
            // The agent profiles, read and write: this device's own settings,
            // so they ride the administrative capability — open to a device
            // the owner granted the whole surface, refused without it.
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
            // The delegation switch, read and write: the same setting-store
            // rule, the same capability. It decides whether this machine's
            // agents may answer their children's permission cards.
            ClientMessage::DelegationGet { id: 1 },
            ClientMessage::DelegationSet {
                id: 1,
                enabled: true,
            },
            // The rest of the surface that used to be denied to every peer:
            // the session verbs outside the view/send pair, the read half of
            // the deposit, the watch set, the project/workspace/provider
            // reads, the status pair and the journal read. One list, because
            // they are one grant.
            ClientMessage::SessionAttachmentRead {
                id: 1,
                reference: devboule_protocol::AttachmentReference {
                    session_id: "s.a.1".to_string(),
                    digest: "b".repeat(64),
                    stored_bytes: 512,
                },
            },
            ClientMessage::SessionDetach {
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
            ClientMessage::SessionsWatch { id: 1 },
            ClientMessage::SessionsUnwatch { id: 1 },
            ClientMessage::SessionsPresence {
                id: 1,
                focused_session_id: None,
                app_visible: true,
            },
            ClientMessage::JournalRetentionGet { id: 1 },
            ClientMessage::ProjectsList { id: 1 },
            ClientMessage::WorkspacesList {
                id: 1,
                project_id: "p.1".to_string(),
            },
            ClientMessage::ProvidersList { id: 1 },
            ClientMessage::Status { id: 1 },
            ClientMessage::DaemonDiagnostics { id: 1 },
        ];
        assert_eq!(
            denied.len() + ALWAYS_ALLOWED_VARIANTS.len(),
            VARIANT_COUNT,
            "the parity walk must cover every variant that is not always allowed"
        );
        for role in [PeerRole::Client, PeerRole::Daemon] {
            for request in &denied {
                match permission_model_denial(request) {
                    // The three permission-model acts: the one thing the parity
                    // decision does not open, in every set.
                    Some(reason) => {
                        assert_eq!(
                            peer_allows(role, &default_caps(), request),
                            PeerDecision::Deny(reason),
                            "{role} may not change the trusted set through {request:?}"
                        );
                        assert_eq!(
                            peer_allows(role, &operational_caps(), request),
                            PeerDecision::Deny(reason),
                            "{role} holding no `admin` is refused {request:?} for the local reason"
                        );
                    }
                    // Everything else: open to a device holding the
                    // administrative capability, refused with that capability's
                    // name — not the act's name, which is not what would open
                    // it — to one that does not.
                    None => {
                        assert_eq!(
                            peer_allows(role, &default_caps(), request),
                            PeerDecision::Allow,
                            "{role} holding a new pairing's grant must be able to ask {request:?}"
                        );
                        assert_eq!(
                            peer_allows(role, &operational_caps(), request),
                            PeerDecision::Deny(CAP_ADMIN),
                            "{role} without `admin` must still be refused {request:?}"
                        );
                    }
                }
            }
        }
    }

    /// The twelve frames no `Deny` arm has ever covered: the handshake pair and
    /// the ten act-named arms. Spelled as wire names so the walk above can prove
    /// it covers every *other* variant — with `VARIANT_COUNT` that is a closed
    /// statement, not a guess.
    const ALWAYS_ALLOWED_VARIANTS: [&str; 12] = [
        "Hello",
        "Ping",
        "SessionsList",
        "DevicesList",
        "PeerAgentsList",
        "SessionAttach",
        "SessionCreate",
        "SessionSend",
        "AgentMessageSend",
        "SessionDeposit",
        "SessionPermissionRespond",
        "SessionSetMode",
    ];

    /// The five wire variants of the three permission-model acts — start or
    /// complete a pairing, change a device's capability set, revoke a device —
    /// with the reason each is refused under. `None` for every other request,
    /// which under the parity rule is exactly "the administrative capability
    /// opens it". Deliberately a second closed match and not a reading of
    /// `matrix_row`: a row and the code disagreeing is the failure this
    /// classifier must be able to report.
    fn permission_model_denial(request: &ClientMessage) -> Option<&'static str> {
        match request {
            ClientMessage::PairingStart { .. } => Some("pairing.start"),
            ClientMessage::PairingComplete { .. } => Some("pairing.complete"),
            ClientMessage::PairingConfirm { .. } => Some("pairing.confirm"),
            ClientMessage::PeerSetCaps { .. } => Some("peer.set_caps"),
            ClientMessage::PeerRevoke { .. } => Some("peer.revoke"),
            _ => None,
        }
    }

    /// §8b A4/A5/R3 as the operational half of the wall: the status report and
    /// the diagnostics bundle are local information, and they ride the
    /// administrative capability like the rest of the surface no act names.
    #[test]
    fn status_and_diagnostics_ride_the_administrative_capability() {
        for role in [PeerRole::Client, PeerRole::Daemon] {
            let status = ClientMessage::Status { id: 1 };
            let diagnostics = ClientMessage::DaemonDiagnostics { id: 1 };
            for request in [&status, &diagnostics] {
                assert_eq!(
                    peer_allows(role, &default_caps(), request),
                    PeerDecision::Allow,
                    "{role} holding a new pairing's grant must reach {request:?}"
                );
                assert_eq!(
                    peer_allows(role, &operational_caps(), request),
                    PeerDecision::Deny(CAP_ADMIN),
                    "{role} without `admin` must not reach {request:?}"
                );
            }
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

    /// The verbs that end a child ride the administrative capability, walked
    /// over the closed wire capability table rather than sampled: **no**
    /// capability but `admin` reaches stop or close — through the wire gate or
    /// the tool door — and `admin` reaches both. A seventh capability that
    /// widened this would have to be named here.
    #[test]
    fn only_the_administrative_capability_reaches_stop_or_close() {
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
                let (wire, door) = if cap == CAP_ADMIN {
                    (PeerDecision::Allow, None)
                } else {
                    (PeerDecision::Deny(CAP_ADMIN), Some(CAP_ADMIN))
                };
                assert_eq!(
                    peer_allows(role, &caps, &stop),
                    wire,
                    "{role:?} holding {cap} on SessionStop"
                );
                assert_eq!(
                    peer_allows(role, &caps, &close),
                    wire,
                    "{role:?} holding {cap} on SessionClose"
                );
                assert_eq!(
                    mcp_tool_denial(role, &caps, MCP_STOP_AGENT_TOOL),
                    door,
                    "{role:?} holding {cap} on the stop tool"
                );
                assert_eq!(
                    mcp_tool_denial(role, &caps, MCP_CLOSE_AGENT_TOOL),
                    door,
                    "{role:?} holding {cap} on the close tool"
                );
            }
            // One capability at a time is not "every peer": a peer holding the
            // act-named five is legal, and so is the whole table. Without
            // `admin` the verbs stay refused; with it they open — the parity
            // decision, not an accident of the loop above.
            for (set, wire, door) in [
                (
                    operational_caps(),
                    PeerDecision::Deny(CAP_ADMIN),
                    Some(CAP_ADMIN),
                ),
                (all_caps(), PeerDecision::Allow, None),
            ] {
                assert_eq!(peer_allows(role, &set, &stop), wire);
                assert_eq!(peer_allows(role, &set, &close), wire);
                assert_eq!(mcp_tool_denial(role, &set, MCP_STOP_AGENT_TOOL), door);
                assert_eq!(mcp_tool_denial(role, &set, MCP_CLOSE_AGENT_TOOL), door);
            }
        }
    }

    /// The project-graph tools ride the administrative capability, walked over
    /// the closed capability table rather than sampled. The property is
    /// two-way, which is what makes it the proof and not a sample: **with** the
    /// capability every served tool is reachable at the door, and **without** it
    /// the caller's workspace graph stays closed whatever else the device holds
    /// — alone, as the act-named five, or in any combination. Whose graph is
    /// read comes from the caller's own session row, never from an argument.
    #[test]
    fn the_project_graph_tools_ride_the_administrative_capability() {
        use crate::provider_catalog::{
            MCP_IMPORTERS_TOOL, MCP_IMPORTS_TOOL, MCP_NEIGHBORHOOD_TOOL,
        };
        use devboule_protocol::PEER_CAPS;
        const GRAPH_TOOLS: [&str; 3] =
            [MCP_NEIGHBORHOOD_TOOL, MCP_IMPORTS_TOOL, MCP_IMPORTERS_TOOL];
        for role in [PeerRole::Client, PeerRole::Daemon] {
            for cap in PEER_CAPS {
                let expected = if cap == CAP_ADMIN {
                    None
                } else {
                    Some(CAP_ADMIN)
                };
                for tool in GRAPH_TOOLS {
                    assert_eq!(
                        mcp_tool_denial(role, &caps(&[cap]), tool),
                        expected,
                        "{role:?} holding {cap} on {tool}"
                    );
                }
            }
            // The negative control that matters: every act-named capability at
            // once, and still no graph.
            for tool in GRAPH_TOOLS {
                assert_eq!(
                    mcp_tool_denial(role, &operational_caps(), tool),
                    Some(CAP_ADMIN),
                    "{role:?} holding every act-named capability must not read the project graph through {tool}"
                );
            }
            // And the parity half: with the whole table no served tool is
            // refused at the door — the graph tools included, which is the
            // statement "a paired device with the administrative capability
            // reaches what the app reaches".
            for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
                assert_eq!(
                    mcp_tool_denial(role, &all_caps(), name),
                    None,
                    "{role:?} holding every capability must reach the served tool {name}"
                );
            }
        }
    }

    /// The door judges with `peer_allows`, per tool, for both roles: the role
    /// never decides (the capability set does), the denials name the policy's
    /// own payloads, and the move tool's two halves deny with two different
    /// sentences — the mode half under `send`, the model half under `admin`.
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
            // which needs `admin` — refuses instead.
            assert_eq!(
                mcp_tool_denial(role, &none, MCP_SET_AGENT_PROFILE_TOOL),
                Some(CAP_SEND)
            );
            assert_eq!(
                mcp_tool_denial(role, &caps(&[CAP_SEND]), MCP_SET_AGENT_PROFILE_TOOL),
                Some(CAP_ADMIN)
            );
            assert_eq!(
                mcp_tool_denial(role, &all_caps(), MCP_SET_AGENT_PROFILE_TOOL),
                None,
                "{role:?} holding every capability, `admin` included, applies the whole profile"
            );
            assert_eq!(
                mcp_tool_denial(role, &operational_caps(), MCP_SET_AGENT_PROFILE_TOOL),
                Some(CAP_ADMIN),
                "{role:?} holding every act-named capability is still refused the model half"
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

    /// A row for one of the three permission-model acts: refused to every set,
    /// the whole wire table included, and the refusal says why.
    fn local(reason: &'static str) -> (PeerDecision, PeerDecision, PeerDecision) {
        (
            PeerDecision::Deny(reason),
            PeerDecision::Deny(reason),
            PeerDecision::Deny(reason),
        )
    }

    /// A row for an act one **operational** capability opens: refused without
    /// any capability, open to the act-named five, open to everything.
    fn under(capability: &'static str) -> (PeerDecision, PeerDecision, PeerDecision) {
        (
            PeerDecision::Deny(capability),
            PeerDecision::Allow,
            PeerDecision::Allow,
        )
    }

    /// A row for an act the **administrative** capability opens — the whole
    /// remainder of this device's surface. Refused without it, with the name of
    /// the capability that would open it; open with it, whether or not the
    /// act-named five are held.
    fn administrative() -> (PeerDecision, PeerDecision, PeerDecision) {
        (
            PeerDecision::Deny(CAP_ADMIN),
            PeerDecision::Deny(CAP_ADMIN),
            PeerDecision::Allow,
        )
    }

    /// §8b A9/A11/A12 plus the 2026-09-21 parity decision, as a table: one row
    /// per `ClientMessage` variant, holding the decision a peer gets with **no**
    /// capability, with the five **operational** capabilities, and with **all
    /// six**.
    ///
    /// The middle column is the negative control and the old world: it is what a
    /// device may do once it holds every act-named capability, and it still
    /// refuses both the administrative surface and the permission model. The
    /// third column is the parity decision — everything but the permission model.
    ///
    /// Closed match with no `_` arm, exactly like `peer_allows` itself: a new
    /// variant does not compile until it has a row here. `VARIANT_COUNT` and
    /// `matrix_samples` below are the other half — they fail the test until the
    /// new variant also has a frame to assert the row on.
    fn matrix_row(request: &ClientMessage) -> (PeerDecision, PeerDecision, PeerDecision) {
        match request {
            ClientMessage::Hello(_) | ClientMessage::Ping { .. } => (allow(), allow(), allow()),
            ClientMessage::SessionsList { .. } | ClientMessage::DevicesList { .. } => {
                under(CAP_VIEW)
            }
            // The peer roster is a read, but a disclosure of its own: it
            // rides the roster capability, not `view` — `view` is the weakest
            // grant a pairing can carry, and the roster is the pairing user's
            // whole live surface.
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
            // The read half of the deposit: a content read, so it rides the
            // administrative capability — scope still decides which reference
            // resolves.
            ClientMessage::SessionAttachmentRead { .. } => administrative(),
            ClientMessage::SessionPermissionRespond { .. } => under(CAP_ANSWER_PERMISSIONS),
            ClientMessage::Status { .. } => administrative(),
            ClientMessage::DaemonDiagnostics { .. } => administrative(),
            ClientMessage::Shutdown { .. } => administrative(),
            ClientMessage::SessionDetach { .. } => administrative(),
            ClientMessage::SessionClaim { .. } => administrative(),
            ClientMessage::SessionClose { .. } => administrative(),
            ClientMessage::SessionStop { .. } => administrative(),
            ClientMessage::SessionResize { .. } => administrative(),
            ClientMessage::SessionInterrupt { .. } => administrative(),
            ClientMessage::SessionSetModel { .. } => administrative(),
            ClientMessage::SessionReportAgent { .. } => administrative(),
            ClientMessage::SessionsWatch { .. } => administrative(),
            ClientMessage::SessionsUnwatch { .. } => administrative(),
            ClientMessage::SessionsPresence { .. } => administrative(),
            ClientMessage::SessionResume { .. } => administrative(),
            ClientMessage::SessionDelete { .. } => administrative(),
            ClientMessage::JournalUsage { .. } => administrative(),
            ClientMessage::JournalRetentionGet { .. } => administrative(),
            ClientMessage::JournalRetentionSet { .. } => administrative(),
            ClientMessage::ProjectsList { .. } => administrative(),
            ClientMessage::ProjectAdd { .. } => administrative(),
            ClientMessage::WorkspacesList { .. } => administrative(),
            ClientMessage::WorkspaceCreate { .. } => administrative(),
            ClientMessage::WorkspaceDelete { .. } => administrative(),
            ClientMessage::ProvidersList { .. } => administrative(),
            ClientMessage::ProvidersRefresh { .. } => administrative(),
            ClientMessage::ProviderUpdate { .. } => administrative(),
            ClientMessage::Invoke { .. } => administrative(),
            ClientMessage::PairingStart { .. } => local("pairing.start"),
            ClientMessage::PairingComplete { .. } => local("pairing.complete"),
            ClientMessage::PairingConfirm { .. } => local("pairing.confirm"),
            ClientMessage::PeerRevoke { .. } => local("peer.revoke"),
            ClientMessage::PeerSetCaps { .. } => local("peer.set_caps"),
            ClientMessage::ToolPolicyGet { .. } => administrative(),
            ClientMessage::ToolPolicySet { .. } => administrative(),
            ClientMessage::AgentProfilesGet { .. } => administrative(),
            ClientMessage::AgentProfilesSet { .. } => administrative(),
            ClientMessage::ProviderVocabularyGet { .. } => administrative(),
            ClientMessage::DelegationGet { .. } => administrative(),
            ClientMessage::DelegationSet { .. } => administrative(),
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

    /// §8b A9/A11/A12 plus the parity decision, end to end: every frame, both
    /// roles, no capability, the act-named five, and every capability, against
    /// the table above. The table is closed by the compiler and the frame list
    /// is pinned by `VARIANT_COUNT`, so a new variant cannot arrive without a
    /// decision *and* a frame to check it on.
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
            let (without_caps, with_operational, with_all) = matrix_row(sample);
            for role in [PeerRole::Client, PeerRole::Daemon] {
                assert_eq!(
                    peer_allows(role, &none, sample),
                    without_caps,
                    "{role} with no capability on {}",
                    sample.name()
                );
                assert_eq!(
                    peer_allows(role, &operational_caps(), sample),
                    with_operational,
                    "{role} with every act-named capability on {}",
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
    fn the_deposited_bytes_read_rides_the_administrative_capability() {
        // The read half of the deposit: a *content* read, so it rides the
        // administrative capability while only the list reads ride `view`. The
        // negative control is the point — without `admin`, no combination of
        // the act-named capabilities opens a paired device the bytes this
        // machine stored.
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
                PeerDecision::Deny(CAP_ADMIN)
            );
            assert_eq!(
                peer_allows(role, &operational_caps(), &read()),
                PeerDecision::Deny(CAP_ADMIN),
                "{role:?} holding every act-named capability must not read deposited bytes"
            );
            assert_eq!(peer_allows(role, &all_caps(), &read()), PeerDecision::Allow);
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
