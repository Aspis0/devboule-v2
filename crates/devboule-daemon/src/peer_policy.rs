//! Peer policy: what a paired connection may do, in one closed place.
//!
//! The decision is a **closed match with no `_` arm** over every
//! `ClientMessage` variant. Adding a variant without deciding its peer policy
//! is a compile error, which is the exhaustiveness guarantee the design asks
//! for (`DESIGN-remote-agents.md` §8 R2, §8b A1/A12). A runtime
//! `ALL_SAMPLES` iteration would only restate what the compiler already
//! enforces.
//!
//! The 1a surface is deliberately tiny: a peer of either role may complete
//! the Noise handshake, send `Hello`, `Ping`, `SessionsList` (role-projected
//! at the dispatch site) and `DevicesList`. Everything else, including
//! `Status`, is refused with `CapabilityNotSupported`; liveness for a peer is
//! `Ping`.

use devboule_protocol::{ClientMessage, SessionKind};

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

/// May `role` send `request`?
///
/// The `role` parameter participates in the signature because slice 3 splits
/// `SessionSend`/`SessionAttach` by role; in 1a both roles share one
/// allowlist, and the role only changes the *projection* of `SessionsList`
/// and `DevicesList` at the dispatch site.
pub fn peer_allows(role: PeerRole, request: &ClientMessage) -> PeerDecision {
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

        // Pairing and revocation are local acts. A peer that could start a
        // pairing or revoke another peer would be able to change this device's
        // trusted set, which is exactly what pairing exists to prevent.
        ClientMessage::PairingStart { .. } => PeerDecision::Deny("pairing.start"),
        ClientMessage::PairingComplete { .. } => PeerDecision::Deny("pairing.complete"),
        ClientMessage::PairingConfirm { .. } => PeerDecision::Deny("pairing.confirm"),
        ClientMessage::PeerRevoke { .. } => PeerDecision::Deny("peer.revoke"),
        ClientMessage::PeerSetCaps { .. } => PeerDecision::Deny("peer.set_caps"),

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

        // Not on the deny list, but not on 1a's allowlist either: they land
        // with their own slice (attach/send/modes, slice 3) and are refused
        // until then rather than half-supported.
        ClientMessage::JournalUsage { .. } => PeerDecision::Deny("journal.usage"),
        ClientMessage::JournalRetentionGet { .. } => PeerDecision::Deny("journal.retention.get"),
        ClientMessage::SessionCreate { .. } => PeerDecision::Deny("session.create"),
        ClientMessage::SessionAttach { .. } => PeerDecision::Deny("session.attach"),
        ClientMessage::SessionDetach { .. } => PeerDecision::Deny("session.detach"),
        ClientMessage::SessionClose { .. } => PeerDecision::Deny("session.close"),
        ClientMessage::SessionStop { .. } => PeerDecision::Deny("session.stop"),
        ClientMessage::SessionSend { .. } => PeerDecision::Deny("session.send"),
        ClientMessage::SessionResize { .. } => PeerDecision::Deny("session.resize"),
        ClientMessage::SessionInterrupt { .. } => PeerDecision::Deny("session.interrupt"),
        ClientMessage::SessionSetModel { .. } => PeerDecision::Deny("session.set_model"),
        ClientMessage::SessionSetMode { .. } => PeerDecision::Deny("session.set_mode"),
        ClientMessage::SessionPermissionRespond { .. } => {
            PeerDecision::Deny("session.permission.respond")
        }
        ClientMessage::SessionsWatch { .. } => PeerDecision::Deny("sessions.watch"),
        ClientMessage::SessionsUnwatch { .. } => PeerDecision::Deny("sessions.unwatch"),
        ClientMessage::SessionsPresence { .. } => PeerDecision::Deny("sessions.presence"),
        ClientMessage::ProjectsList { .. } => PeerDecision::Deny("projects.list"),
        ClientMessage::WorkspacesList { .. } => PeerDecision::Deny("workspaces.list"),
        ClientMessage::ProvidersList { .. } => PeerDecision::Deny("providers.list"),
    }
}

/// Whether `mode_id` is a mode that can run without asking the target
/// device's user (`DESIGN-remote-agents.md` §8b A5).
///
/// Not called by 1a's dispatch: nothing reaches a session in this slice. It
/// exists now (with its test) because the list is a policy decision that must
/// live in one place before slice 3 starts asking remote-origin sessions for
/// it.
///
/// The lists are concrete because "prompt skipping" is per provider and is
/// Whether `mode_id` is a mode that can run without asking the target
/// device's user (`DESIGN-remote-agents.md` §8b A5).
///
/// Called from the peer gate's audit path (`server.rs::peer_outcome`): a remote
/// request that names one of these modes is refused and recorded as
/// `prompt_skipping_refused`, so the trail distinguishes an attempt at
/// unattended execution from an ordinary denial. Slice 3, which lets a `Daemon`
/// peer reach a session, uses the same list to refuse the request outright.
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

    #[test]
    fn the_allowlist_is_exactly_the_1a_surface() {
        for role in [PeerRole::Client, PeerRole::Daemon] {
            assert_eq!(peer_allows(role, &ping()), PeerDecision::Allow);
            assert_eq!(
                peer_allows(role, &ClientMessage::SessionsList { id: 1 }),
                PeerDecision::Allow
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
        ];
        for role in [PeerRole::Client, PeerRole::Daemon] {
            for request in &denied {
                assert!(
                    matches!(peer_allows(role, request), PeerDecision::Deny(_)),
                    "{role} may not send {request:?}"
                );
            }
        }
    }

    #[test]
    fn status_is_denied_to_both_roles() {
        for role in [PeerRole::Client, PeerRole::Daemon] {
            assert_eq!(
                peer_allows(role, &ClientMessage::Status { id: 1 }),
                PeerDecision::Deny("status")
            );
            assert_eq!(
                peer_allows(role, &ClientMessage::DaemonDiagnostics { id: 1 }),
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
            assert_eq!(
                peer_allows(role, &ClientMessage::DevicesList { id: 1 }),
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
                assert!(
                    matches!(peer_allows(role, &request), PeerDecision::Deny(_)),
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
}
