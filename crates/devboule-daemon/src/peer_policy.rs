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

/// Why a send from a paired device that carries attachments is refused, for
/// now (`server.rs::session_send`). Temporary: it goes away when the deposit
/// branch's counter lands and `budget_for` has a caller.
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
        ClientMessage::SessionsList { .. } => PeerDecision::Allow,
        // Role-projected at the dispatch site; a `Daemon` peer sees only
        // `{device_id, display_name, role, online}` (design §8b A13).
        ClientMessage::DevicesList { .. } => PeerDecision::Allow,

        // Slice 3: the five session variants a paired device may reach, each
        // under the capability that names the act. `view` is what makes a peer
        // a viewer at all; it is the one capability `validate_caps` will not
        // remove from a `Client` (A11).
        ClientMessage::SessionAttach { .. } => with_capability(caps, CAP_VIEW),
        ClientMessage::SessionCreate { .. } => with_capability(caps, CAP_CREATE_SESSIONS),
        ClientMessage::SessionSend { .. } => with_capability(caps, CAP_SEND),
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
mod tests {
    use super::*;
    use devboule_protocol::OwnerId;

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
