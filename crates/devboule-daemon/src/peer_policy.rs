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
/// pins these four to it so a rename cannot leave the gate enforcing a
/// capability nobody can hold.
pub const CAP_VIEW: &str = "view";
pub const CAP_SEND: &str = "send";
pub const CAP_ANSWER_PERMISSIONS: &str = "answer_permissions";
pub const CAP_CREATE_SESSIONS: &str = "create_sessions";

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

        // Slice 3: the five session variants a paired device may reach, each
        // under the capability that names the act. `view` is what makes a peer
        // a viewer at all; it is the one capability `validate_caps` will not
        // remove from a `Client` (A11).
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
/// and slice 3's refusal (`server.rs::peer_mode_refusal`) refuses the request
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
    match kind {
        SessionKind::Claude => matches!(mode_id, "acceptEdits" | "auto" | "bypassPermissions"),
        SessionKind::Codex => matches!(mode_id, "auto-review" | "full-access"),
        SessionKind::Pi => mode_id == "bypass",
        SessionKind::Acp => false,
        SessionKind::Terminal => false,
    }
}

/// The `unattended` marker for one session: the honest answer to "can this
/// session pass a permission moment with no human answering", derived from
/// the mode the daemon **delivered** (`DESIGN-what-unattended-means.md`).
///
/// This is the sibling of [`prompt_skipping_mode`] — in the same home, keyed
/// the same way, and **never merged with it**: a mode can skip prompts for a
/// peer and still be un-establishable for this marker. The two differ in
/// shape where their knowledge differs:
///
/// - **Route A — the daemon answers itself.** A delivered mode carrying one
///   of the provider-agnostic ids `provider_catalog::mode_is_auto_answered`
///   lists is answered by the daemon's own broker, whatever *agent* family
///   the session belongs to; the shared helper below is the one list, and
///   every arm's dictionary sits behind it. A terminal is **not** an agent
///   family: the broker's auto-answer call sites cover the agent clients
///   only, and a terminal has no permission mechanism at all, so there is no
///   permission moment for anything to answer and no mode id — including a
///   route-A id — can make one exist. The kind match below, not this list,
///   decides a terminal, and it says `no` (audit R2b-1 §3.1: a terminal
///   created with `mode: "bypass"` used to answer `yes` because this check
///   ran first).
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
///   route-A ids the answer is `unknown`. The same arm covers what nobody
///   said: an absent or empty delivered mode is `unknown` for a family the
///   daemon does not set a mode for, and a terminal — which has no
///   permission mechanism at all, and never had one to be told about —
///   carries `no`, exactly what the collapsed `bool` this marker replaces
///   recorded for it. A user-defined provider's child from a config file
///   lands in this arm because `session_kind_for(provider)` — the one
///   provider-name match on this path, read at the birth before this
///   function is consulted — maps every name that is not one of the three
///   authored families to [`SessionKind::Acp`], whose arm is `unknown`: no
///   name beyond the three is ever *interpreted*, and an unrecognised one
///   fails toward the honest answer, not toward `no`.
pub fn unattended_mode(
    kind: SessionKind,
    delivered_mode: Option<&str>,
) -> devboule_protocol::UnattendedState {
    use devboule_protocol::UnattendedState;
    let delivered_mode = delivered_mode.filter(|mode| !mode.is_empty());
    // Route A before the kind match, but never for a terminal: `Yes` claims
    // the daemon's own broker will answer this child's permission moments,
    // and a terminal has none to answer — no mode id delivered to it can
    // make the claim true, so the one-condition gate keeps the kind match's
    // `no` the rule for it instead of a fall-through an id could outrank.
    if kind != SessionKind::Terminal
        && delivered_mode.is_some_and(crate::provider_catalog::mode_is_auto_answered)
    {
        return UnattendedState::Yes;
    }
    match kind {
        SessionKind::Claude => crate::claude_view::unattended_answer(delivered_mode),
        SessionKind::Codex => crate::codex_view::unattended_answer(delivered_mode),
        // `pi_client` is `session`'s submodule; the dictionary is re-exported
        // beside the module declarations there, the way `session_runtime`'s
        // types are.
        SessionKind::Pi => crate::session::pi_unattended_answer(delivered_mode),
        SessionKind::Acp => UnattendedState::Unknown,
        SessionKind::Terminal => UnattendedState::No,
    }
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
    if kind == SessionKind::Acp {
        return Some(ACP_MODES_UNVETTED_REFUSED);
    }
    prompt_skipping_mode(kind, mode_id).then_some(PROMPT_SKIPPING_REFUSED)
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

/// What the daemon knows about the far end of one connection. `Local` keeps
/// the kernel-derived `PeerIdentity` that `session.rs` reads through
/// `ConnHandle.peer`; `Remote` is the Noise-authenticated peer, and its
/// `paired_by_user` is copied from the `peers` row at handshake time so no
/// journal lookup happens at dispatch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnPeer {
    Local(crate::agent_report::PeerIdentity),
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
            Self::Local(_) => None,
            Self::Remote { device_id, .. } => Some(device_id),
        }
    }

    pub fn role(&self) -> Option<PeerRole> {
        match self {
            Self::Local(_) => None,
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
        ])
    }

    #[test]
    fn the_four_capability_names_are_the_protocol_list() {
        // The gate enforces these strings; the wire accepts exactly
        // `PEER_CAPS`. Pinning them here means a rename on either side fails
        // loudly instead of leaving a capability nobody can hold.
        let mut named = [
            CAP_VIEW,
            CAP_SEND,
            CAP_ANSWER_PERMISSIONS,
            CAP_CREATE_SESSIONS,
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
        }
    }

    /// The number of `ClientMessage` variants at this commit. The closed match
    /// in `every_variant_is_listed` breaks the build when a variant is added;
    /// this number is what then fails
    /// `the_capability_matrix_covers_every_client_frame` until the new variant
    /// also has a sample to assert its row on. Both halves are needed: the
    /// match proves the *decisions* are complete, the count proves the
    /// *frames* are.
    pub(crate) const VARIANT_COUNT: usize = 49;

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
        let local = ConnPeer::Local(crate::agent_report::PeerIdentity {
            user: "S-1-5-21-1".to_string(),
            pid: 42,
        });
        assert!(local.device_id().is_none());
        assert!(local.role().is_none());
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
