//! Devboule daemon protocol: wire types shared by the app and the daemon.
//!
//! This crate has **no I/O**, no tokio, and no Tauri. Both sides depend on it
//! so a request/response/event/error/handshake disagreement is a compile error
//! rather than a runtime deserialization failure in a user's hands.
//!
//! # Version compatibility
//!
//! Handshake is bidirectional. The client states the highest protocol version
//! it speaks and the lowest it can accept; the daemon does the same. The
//! agreed version is `min(client.protocol_version, daemon.protocol_version)`.
//! Handshake fails if that value is below either side's minimum.
//!
//! - **Daemon newer than the app**, ranges overlap: they speak the app's
//!   version. Extra daemon capabilities are ignored.
//! - **Daemon older than the app**, ranges overlap: they speak the daemon's
//!   version. The app must not send ops the daemon did not advertise.
//! - **No overlap**: the daemon replies with
//!   [`ErrorCode::ProtocolVersionMismatch`] and closes. The error names both
//!   versions and which binary to update. Neither side may hang or try to
//!   parse the rest of the stream as the other version.
//!
//! M3a speaks only version [`PROTOCOL_VERSION`] (1), with
//! [`PROTOCOL_MIN_VERSION`] also 1. Bumping `PROTOCOL_MIN_VERSION` is how a
//! future daemon or app *drops* an old dialect; until then, a newer daemon
//! must still accept version 1.
//!
//! Capabilities are an open string set, independently negotiated as the
//! intersection of what both sides listed. Unknown capability names MUST be
//! ignored (not a handshake failure). An RPC whose capability was not agreed
//! returns [`ErrorCode::CapabilityNotSupported`].
//!
//! # Framing (transport, not this crate)
//!
//! The byte transport is **newline-delimited compact JSON**. It is not defined
//! here so a Unix socket can replace a named pipe without touching these
//! types. See `devboule-daemon` for the choice and the max-frame cap.
//!
//! # Session identifiers
//!
//! A session id is an opaque string that **carries the owner**. New ids use
//! [`compose_session_id`]: `s.{owner}.{unique}`. The M2 in-process terminal
//! still mints `session-{pid}-{counter}`; [`validate_session_id`] accepts both.
//!
//! # `detach`, `close`, `stop`
//!
//! Three distinct operations. See [`ClientMessage`] variants. Collapsing any
//! two would change the protocol's meaning, not add a field later.
//!
//! # Cursor and generation
//!
//! Replay uses [`Cursor`]: `generation` names the backing process instance,
//! `seq` is the last output sequence the client has for that instance.
//! Replaying across a generation change is a
//! [`ErrorCode::SessionGenerationMismatch`], never a silent continuation.
//!
//! # Screen snapshots (M3.5)
//!
//! On attach the daemon sends a [`SessionEvent::Snapshot`] with the current
//! emulator state instead of replaying past frames. Its `as_of_seq` field
//! is the sequence boundary on **application to the emulator**; the type's
//! documentation carries the invariant and the reason it is not allowed to
//! drift to pipe write, journal commit, or client receipt.
//!
//! # Idempotency
//!
//! `session_create`, `session_send`, and `session_permission_respond` carry an
//! optional [`idempotency_key`]. Keys are remembered per owner for
//! [`IDEMPOTENCY_TTL_SECS`] seconds (capped at [`IDEMPOTENCY_MAX_ENTRIES`]).
//! A retry with the same key and the same payload returns the original
//! result; a retry with the same key and a different payload returns
//! [`ErrorCode::IdempotencyConflict`].

mod capability;
mod error;
mod handshake;
mod ids;
mod messages;
mod plugin;
mod session;
#[cfg(test)]
mod session_event_guard;

pub use capability::{intersect_capabilities, Capability};
pub use error::{ErrorCode, ErrorDetails, WireError};
pub use handshake::{negotiate, ClientHello, DaemonHello, Negotiation};
pub use ids::{
    compose_session_id, validate_idempotency_key, validate_owner_token, validate_session_id,
    OwnerId,
};
pub use messages::{
    ClientMessage, DaemonMessage, DaemonStatusBody, JournalLimits, JournalRetention,
    JournalSessionUsage, JournalStats, JournalUsage, ProviderInfo, RetentionLimit, RetentionPatch,
    RetentionSource, SessionEventEnvelope, Unreclaimable,
};
pub use plugin::WorkspaceRootBody;
pub use session::{
    cursor_replay_ok, AgentActivityState, Attention, AttentionReason, AvailableCommandView, Cursor,
    CursorShape, PermissionEnvVar, PermissionOption, PermissionOutcome, Persistence,
    PersistenceKind, ResumeResult, ScreenCursor, Session, SessionEvent, SessionKind,
    SessionModeStateView, SessionModeView, SessionModel, SessionModelEffort, SessionState,
    SessionStateSnapshot, ToolLocation, TranscriptIntegrity, TurnUsage,
};

/// Current protocol dialect spoken by this crate.
pub const PROTOCOL_VERSION: u32 = 1;
/// Oldest dialect this crate still accepts. Equal to [`PROTOCOL_VERSION`] in
/// M3a; a future bump is how an old dialect is dropped.
pub const PROTOCOL_MIN_VERSION: u32 = 1;

/// Well-known capability names. These are strings on the wire so a peer that
/// does not know a name can still complete the handshake.
pub mod caps {
    pub const PING: &str = "ping";
    pub const STATUS: &str = "status";
    pub const SHUTDOWN: &str = "shutdown";
    /// Session RPCs (create/attach/detach/close/stop/send/…). Advertised
    /// from M3b so the app and daemon agree to speak them.
    pub const SESSIONS: &str = "sessions";
    /// Conversation journal. Advertised in M3c.
    pub const JOURNAL: &str = "journal";

    /// Plugin-backend tenant. The host grants these at handshake from what
    /// the plugin manifest requested; a name the host does not know is
    /// ignored, not a handshake failure. Same open-set rule as the daemon.
    pub const WORKSPACE_ROOT: &str = "workspace.root";
    pub const CITY_GET: &str = "city.get";
    pub const FINDINGS_GET: &str = "findings.get";
    pub const FINDING_INSPECT: &str = "finding.inspect";
    pub const ORACLE_SEARCH: &str = "oracle.search";
    pub const GRAPH_IMPORTS: &str = "graph.imports";
    pub const SESSIONS_WATCH: &str = "sessions.watch";
    pub const AGENT_RUN: &str = "agent.run";
    pub const TYPED_PERMISSIONS: &str = "typed_permissions";
}

/// How long the daemon remembers an idempotency key, in seconds.
pub const IDEMPOTENCY_TTL_SECS: u64 = 15 * 60;
/// Maximum remembered idempotency entries per daemon process. Evict oldest.
pub const IDEMPOTENCY_MAX_ENTRIES: usize = 4096;

/// Compact JSON frames larger than this are a protocol error (1 MiB).
///
/// The largest ordinary frame is a screen snapshot: a dense 200x50 screen
/// where every cell repaints its 24-bit colours escapes to roughly 490 KiB
/// of JSON. That fits, but it is orders of magnitude larger than a typical
/// output frame — see the frame-cap test in the `session` module before
/// assuming snapshots are always small.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
/// Maximum serialized JSON payload accepted by the plugin invoke command.
pub const MAX_PLUGIN_PAYLOAD_BYTES: usize = 1024 * 1024;

pub fn plugin_payload_within_limit(payload: Option<&serde_json::Value>) -> bool {
    payload
        .map(|value| {
            serde_json::to_vec(value)
                .map(|bytes| bytes.len() <= MAX_PLUGIN_PAYLOAD_BYTES)
                .unwrap_or(false)
        })
        .unwrap_or(true)
}
/// Capabilities this crate's daemon and app currently serve.
///
/// Named `m3a_*` because the handshake helpers were introduced in M3a; M3b
/// adds [`caps::SESSIONS`] without changing the helper names so a peer
/// built against this crate still calls the same constructors.
pub fn m3a_daemon_capabilities() -> Vec<Capability> {
    let mut capabilities = vec![
        Capability::new(caps::PING),
        Capability::new(caps::STATUS),
        Capability::new(caps::SHUTDOWN),
        Capability::new(caps::SESSIONS),
        Capability::new(caps::JOURNAL),
    ];
    capabilities.push(Capability::new(caps::TYPED_PERMISSIONS));
    capabilities
}

/// Capabilities the M3a app client offers.
pub fn m3a_client_capabilities() -> Vec<Capability> {
    let mut capabilities = vec![
        Capability::new(caps::PING),
        Capability::new(caps::STATUS),
        Capability::new(caps::SHUTDOWN),
        Capability::new(caps::SESSIONS),
        Capability::new(caps::JOURNAL),
    ];
    capabilities.push(Capability::new(caps::TYPED_PERMISSIONS));
    capabilities
}

/// Capabilities a plugin backend advertises today. The host may grant a
/// subset. Later plugin work adds names here; unknown names on either side
/// still complete the handshake.
pub fn plugin_backend_capabilities() -> Vec<Capability> {
    vec![
        Capability::new(caps::PING),
        Capability::new(caps::WORKSPACE_ROOT),
        Capability::new(caps::CITY_GET),
        Capability::new(caps::FINDINGS_GET),
        Capability::new(caps::FINDING_INSPECT),
    ]
}

/// The invoke method for a capability is the capability name. The host
/// refuses a method that was not in the negotiated set.
pub fn invoke_method_capability(method: &str) -> &str {
    method
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_one_and_min_matches() {
        assert_eq!(PROTOCOL_VERSION, 1);
        assert_eq!(PROTOCOL_MIN_VERSION, 1);
    }

    #[test]
    fn daemon_and_client_advertise_sessions() {
        let daemon = m3a_daemon_capabilities();
        let client = m3a_client_capabilities();
        assert!(daemon.iter().any(|cap| cap.as_str() == caps::SESSIONS));
        assert!(client.iter().any(|cap| cap.as_str() == caps::SESSIONS));
        assert!(daemon.iter().any(|cap| cap.as_str() == caps::JOURNAL));
        assert!(client.iter().any(|cap| cap.as_str() == caps::JOURNAL));
        assert_eq!(daemon, client);
    }

    #[test]
    fn plugin_backend_is_a_second_tenant_not_a_daemon_capability() {
        let daemon = m3a_daemon_capabilities();
        let plugin = plugin_backend_capabilities();
        assert!(
            !daemon
                .iter()
                .any(|cap| cap.as_str() == caps::WORKSPACE_ROOT),
            "workspace.root belongs to the plugin tenant, not the daemon"
        );
        assert!(plugin.iter().any(|cap| cap.as_str() == caps::PING));
        assert!(plugin
            .iter()
            .any(|cap| cap.as_str() == caps::WORKSPACE_ROOT));
        assert!(plugin.iter().any(|cap| cap.as_str() == caps::CITY_GET));
        assert!(plugin.iter().any(|cap| cap.as_str() == caps::FINDINGS_GET));
        assert!(plugin
            .iter()
            .any(|cap| cap.as_str() == caps::FINDING_INSPECT));
        assert!(!plugin.iter().any(|cap| cap.as_str() == caps::STATUS));
        assert!(!plugin.iter().any(|cap| cap.as_str() == caps::SESSIONS));
        assert_eq!(
            invoke_method_capability("workspace.root"),
            caps::WORKSPACE_ROOT
        );
        assert_eq!(invoke_method_capability("findings.get"), caps::FINDINGS_GET);
        assert_eq!(
            invoke_method_capability("finding.inspect"),
            caps::FINDING_INSPECT
        );
    }

    #[test]
    fn plugin_payloads_are_capped_before_framing() {
        let small = serde_json::Value::String("x".repeat(MAX_PLUGIN_PAYLOAD_BYTES - 16));
        let large = serde_json::Value::String("x".repeat(MAX_PLUGIN_PAYLOAD_BYTES));
        assert!(plugin_payload_within_limit(Some(&small)));
        assert!(!plugin_payload_within_limit(Some(&large)));
        assert!(plugin_payload_within_limit(None));
    }
}
